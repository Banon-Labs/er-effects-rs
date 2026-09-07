//! Post-release cover watch + in-game-menu open stamps + the log/telemetry clock map.
//!
//! Moved verbatim from er-quickload `telemetry/cover_after_release.rs` with the loading-cover
//! crate extraction. The product functions it reads -- the debug-log sink, the JSON emitter, the
//! log clock, the boot-view clock and the game's own cover-plate read -- cross the seam through
//! [`crate::loading_cover_host`]; every call site keeps its original name.

use crate::loading_cover_host::{
    boot_view_epoch_ms_if_anchored, fake_loading_screen_visible, process_log_elapsed_ms,
    push_json_usize,
};
use crate::prelude::*;

// Post-release cover watch (user report 2026-08-22: "pressing Escape quickly after the loading
// screen fades out -- while the location banner is still on screen -- makes the loading screen and
// portrait briefly reappear and tear down"; timing-dependent on opening the menu swiftly).
//
// Why this exists at all. The run that reproduced the defect, br-20260822-184123-fa3d, left zero
// trace in the DLL log. Not a weak trace -- none. Every aggregate counter said the cover was long
// dead (`BOOT_VIEW_STOPPED` latched and never cleared, `oracle_boot_view_epoch_seq=0`,
// `oracle_boot_view_fps_bail_resumes=0`), and the only per-frame oracle we own,
// [`native_ls_exposure_record`], switches itself off in this exact window: it returns unless the
// game's `CS::LoadingScreen` ticked within 250 ms, and in that run the native screen stopped
// ticking ~2.2 s before our cover stopped. The one moment worth watching was the one moment
// nothing watched. This is the mirror of that function for the other side of the release.
//
// What it measures, and what each answer means:
//
//   oracle_cover_after_release_samples == 0
//       the watch never opened. Every zero below is then meaningless -- read this first. It is 0
//       when no cover window has stopped yet, or when every stop is older than the watch span.
//   oracle_cover_plate_visible_after_release > 0
//       the game's own `CSFakeLoadingScreenImp` cover plate read visible on a frame that reached
//       Present, after our cover had already released. That is the surface the user described, and
//       it is the game's, not ours. `_max_run` separates a brief reappearance (short run) from a
//       plate that simply never went down (one run as long as the watch).
//   oracle_native_ls_activity_after_release_updates / _fadeouts > 0
//       the game's own loading screen kept working past our release: `CS::LoadingScreen::Update`
//       ticks and Scaleform fade-out stamps, measured as deltas against baselines snapshotted at
//       the stop. A legitimate new load starting inside the watch span (a death, a fast travel)
//       would also show here, so read these next to `oracle_portrait_loadwin_history` rather than
//       alone.
//   oracle_in_game_menu_open_first_ms_after_cover_stop
//       when the user opened the in-world pause/System menu after the cover released -- the Escape
//       press the report is about. Subtract it from
//       `oracle_cover_plate_visible_after_release_first_ms` and the answer is the press-to-
//       reappearance interval, which used to be a hand pairing of a log line against an oracle. 0
//       means no menu open was seen past a cover stop, which makes any plate hit above not the
//       reported defect. `_edges` / `_last_ms` cover the whole session; see
//       `in_game_menu_note_run_tick` for what the edge is and what it costs in precision.
//   oracle_log_epoch_offset_ms
//       the constant separating this log's `[+Nms]` prefixes from every telemetry `*_ms` above:
//       `log = telemetry + offset`. Emitted so nobody measures it again by hand.
//
// This changes nothing that is drawn. It is telemetry, and it deliberately does not attempt a
// pixel probe: whether the plate is up is answerable from RAM, and a full-frame readback in the
// Present path is not something to add before the cheap question has been asked.
//
// Cost. Two `ReadProcessMemory` self-reads per sampled frame (the fault-guarded singleton walk in
// `fake_loading_screen_visible`) and a handful of relaxed atomics. Bounded by
// `COVER_AFTER_RELEASE_WATCH_MS` so gameplay does not pay for it forever: after a stop the watch
// runs for that span and then goes quiet until the next cover window stops.

/// How long past a cover stop the watch samples. The reported defect happens while the location
/// banner is still up -- seconds, not minutes -- so this is generous by an order of magnitude and
/// still bounds the per-frame cost to one span per cover window. It is a cost bound, not a
/// judgement about when a spurious cover may occur.
const COVER_AFTER_RELEASE_WATCH_MS: u64 = 30_000;

/// How long a break in `02_000_IngameTop` `MenuWindowJob::Run` ticks separates one menu session
/// from the next.
///
/// The job runs once per presented frame while that menu is up. Measured cadence in the shipped
/// DLL's own log of a real session (2026-08-22 11:41:30, game-dir `er-quickload-autoload-debug.log`):
/// 47-62 ms apart early, 21-23 ms apart later -- i.e. the frame period at ~16 fps and ~45 fps. One
/// second is more than an order of magnitude above that, so a slow frame or a hitch cannot be
/// mistaken for the menu having closed and reopened.
///
/// The threshold is deliberately biased toward under-counting: two genuine opens less than a second
/// apart read as one. That is the safe direction for a stamp meant to be trusted -- a missing edge
/// is visible as a zero, a fabricated one would be read as the user's press.
const IN_GAME_MENU_TICK_GAP_MS: u64 = 1_000;

/// How far the telemetry clock may advance across the clock-map's own pair of readings before the
/// pair is discarded as not simultaneous. 1 ms is the resolution of both clocks, so this accepts a
/// clean reading and rejects any thread delay big enough to show up in the answer.
const CLOCK_MAP_MAX_BRACKET_MS: u64 = 1;

/// Sample the post-release state for one presented frame. Called from
/// [`native_ls_exposure_record`] on exactly the frames that function declines to judge -- the ones
/// where the native loading screen is stale -- so the two never double-count and the existing
/// exposure accounting is untouched.
///
/// Runs on the game render thread inside Present: no allocation, no scan, no lock.
pub fn cover_after_release_record(base: usize, now_ms: u64) {
    use er_telemetry_core::counters::{
        BOOT_VIEW_STOP_LS_FADEOUT_BASELINE, BOOT_VIEW_STOP_LS_UPDATE_BASELINE, BOOT_VIEW_STOP_MS,
        BOOT_VIEW_STOPPED, COVER_PLATE_AFTER_RELEASE_SAMPLES, COVER_PLATE_VISIBLE_AFTER_RELEASE,
        COVER_PLATE_VISIBLE_AFTER_RELEASE_CUR_RUN, COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS,
        COVER_PLATE_VISIBLE_AFTER_RELEASE_LAST_MS, COVER_PLATE_VISIBLE_AFTER_RELEASE_MAX_RUN,
        LOADING_SCREEN_GFX_FADEOUT_HITS, LOADING_SCREEN_UPDATE_HITS,
        NATIVE_LS_ACTIVITY_AFTER_RELEASE_FADEOUTS, NATIVE_LS_ACTIVITY_AFTER_RELEASE_FIRST_MS,
        NATIVE_LS_ACTIVITY_AFTER_RELEASE_UPDATES,
    };
    // `BOOT_VIEW_STOPPED`, not `BOOT_VIEW_FADE_COMPLETE_MS`: the FPS-bail exit never sets the
    // latter, so keying on it would leave a bail-stopped window unwatched.
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) == 0 {
        return;
    }
    let stop_ms = BOOT_VIEW_STOP_MS.load(Ordering::SeqCst) as u64;
    if stop_ms == 0 || now_ms.saturating_sub(stop_ms) > COVER_AFTER_RELEASE_WATCH_MS {
        return;
    }
    COVER_PLATE_AFTER_RELEASE_SAMPLES.fetch_add(1, Ordering::SeqCst);
    let now_usize = now_ms as usize;
    // The game's own loading screen, measured as a delta so a counter that is cumulative for the
    // whole process still answers "did it do anything AFTER we let go".
    let updates = LOADING_SCREEN_UPDATE_HITS
        .load(Ordering::SeqCst)
        .saturating_sub(BOOT_VIEW_STOP_LS_UPDATE_BASELINE.load(Ordering::SeqCst));
    let fadeouts = LOADING_SCREEN_GFX_FADEOUT_HITS
        .load(Ordering::SeqCst)
        .saturating_sub(BOOT_VIEW_STOP_LS_FADEOUT_BASELINE.load(Ordering::SeqCst));
    NATIVE_LS_ACTIVITY_AFTER_RELEASE_UPDATES.fetch_max(updates, Ordering::SeqCst);
    NATIVE_LS_ACTIVITY_AFTER_RELEASE_FADEOUTS.fetch_max(fadeouts, Ordering::SeqCst);
    if updates + fadeouts > 0 {
        let _ = NATIVE_LS_ACTIVITY_AFTER_RELEASE_FIRST_MS.compare_exchange(
            0,
            now_usize,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    // The decisive read: is the game's render-pipeline cover plate up on this presented frame?
    let visible = unsafe { fake_loading_screen_visible(base) };
    if !visible {
        COVER_PLATE_VISIBLE_AFTER_RELEASE_CUR_RUN.store(0, Ordering::SeqCst);
        return;
    }
    let n = COVER_PLATE_VISIBLE_AFTER_RELEASE.fetch_add(1, Ordering::SeqCst) + 1;
    let run = COVER_PLATE_VISIBLE_AFTER_RELEASE_CUR_RUN.fetch_add(1, Ordering::SeqCst) + 1;
    COVER_PLATE_VISIBLE_AFTER_RELEASE_MAX_RUN.fetch_max(run, Ordering::SeqCst);
    COVER_PLATE_VISIBLE_AFTER_RELEASE_LAST_MS.store(now_usize, Ordering::SeqCst);
    let _ = COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS.compare_exchange(
        0,
        now_usize,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    // First frame of each run only -- a sustained plate is one event, and this is the Present hook.
    if run == 1 {
        append_autoload_debug(format_args!(
            "COVER-AFTER-RELEASE #{n}: the game's own CSFakeLoadingScreenImp plate is VISIBLE at {now_usize}ms, {}ms after our cover stopped (stop_reason={} native_ls_updates_since_stop={updates} fadeouts_since_stop={fadeouts}) -- this surface is the GAME's, not our compositor's",
            now_ms.saturating_sub(stop_ms),
            er_telemetry_core::counters::BOOT_VIEW_STOP_REASON.load(Ordering::SeqCst),
        ));
    }
}

/// Note one `02_000_IngameTop` `MenuWindowJob::Run` tick, and stamp the ones that start a menu
/// session.
///
/// Why this exists. The watch above can say a cover plate came back at ms X. The user's report says
/// the trigger is pressing Escape quickly after a load, while the location banner is still up.
/// Nothing stamped that press, so X could only be tied to it by hand -- on a defect whose entire
/// signature is the interval between the two.
///
/// Why this signal. `02_000_IngameTop` is the game's own resource name for the in-world pause/System
/// menu (the window `request_open_ingame_menu` opens via `CSPopupMenu+0x121`), and its
/// `MenuWindowJob` runs once per presented frame while that menu is up and not at all otherwise.
/// Both halves are ground truth from the shipped DLL's own log of a real session (2026-08-22
/// 11:41:30 run): zero `02_000_IngameTop` `MenuWindowJob::Run` lines across the first 39.9 s of
/// boot, character load and gameplay, then a first line at `[+39905ms]` carrying `prev=0x0`,
/// followed by one line every ~20-60 ms until the log cap. So a tick after a gap is an open.
///
/// Called from the product `MenuWindowJob::Run` post-hook, which already resolves the resource name
/// to maintain `SYSTEM_QUIT_INGAME_TOP_WINDOW` -- no new hook, no new task, and no per-frame work
/// added anywhere that was not already running.
pub fn in_game_menu_note_run_tick(job: usize, window: usize) {
    use er_telemetry_core::counters::{
        BOOT_VIEW_STOP_MS, BOOT_VIEW_STOPPED, IN_GAME_MENU_OPEN_EDGES, IN_GAME_MENU_OPEN_FIRST_MS,
        IN_GAME_MENU_OPEN_FIRST_MS_AFTER_COVER_STOP, IN_GAME_MENU_OPEN_LAST_MS,
        IN_GAME_MENU_OPEN_MS_FIRST_N, IN_GAME_MENU_OPEN_MS_FIRST_N_LEN, IN_GAME_MENU_RUN_LAST_MS,
    };
    // Read the boot-view clock without starting it: every telemetry `*_ms` is measured from that
    // epoch, so anchoring it here -- from a menu hook, for a stamp -- would move the origin of the
    // whole run's timeline. Before it exists there is no cover window to correlate against anyway.
    let Some(now_ms) = boot_view_epoch_ms_if_anchored() else {
        return;
    };
    let now = now_ms.max(1) as usize;
    let prev = IN_GAME_MENU_RUN_LAST_MS.swap(now, Ordering::SeqCst) as u64;
    if prev != 0 && now_ms.saturating_sub(prev) <= IN_GAME_MENU_TICK_GAP_MS {
        // Still the same menu session: this is a later frame of a menu that was already up.
        return;
    }
    let n = IN_GAME_MENU_OPEN_EDGES.fetch_add(1, Ordering::SeqCst) + 1;
    IN_GAME_MENU_OPEN_LAST_MS.store(now, Ordering::SeqCst);
    // Stamp every early edge, above the cover-stop test (2026-08-22, second round). The latch below
    // only takes edges that land after a cover stop, and the press the user is reporting lands
    // before it -- during the release fade. In run br-20260822-184123-fa3d that is exactly what
    // happened: the after-stop latch recorded the later control press at 36537 ms and edge #1, the
    // fast one that caused the defect, left no timestamp anywhere. These two writes are what make a
    // press that precedes the stop recoverable at all.
    let _ = IN_GAME_MENU_OPEN_FIRST_MS.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst);
    if n <= IN_GAME_MENU_OPEN_MS_FIRST_N_LEN {
        IN_GAME_MENU_OPEN_MS_FIRST_N[n - 1].store(now, Ordering::SeqCst);
    }
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) == 0 {
        // A menu opened, but no cover has released, so this cannot be the press in the report and
        // there is nothing to correlate it against. Counted, not logged: opening the menu is
        // ordinary play and the log must not narrate it.
        return;
    }
    let first = IN_GAME_MENU_OPEN_FIRST_MS_AFTER_COVER_STOP
        .compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    let stop_ms = BOOT_VIEW_STOP_MS.load(Ordering::SeqCst) as u64;
    // One line per open past a cover stop -- rare by construction (the menu is not open during
    // gameplay and the tick is de-duplicated above), so this cannot become an IO storm.
    append_autoload_debug(format_args!(
        "IN-GAME MENU OPEN #{n}: the game's own 02_000_IngameTop MenuWindowJob started running at {now}ms, {}ms after our cover stopped (boot_view_stop_ms={stop_ms} first_after_cover_stop={first} job=0x{job:x} window=0x{window:x}) -- correlate this against oracle_cover_plate_visible_after_release_first_ms",
        now_ms.saturating_sub(stop_ms),
    ));
}

/// State once, in the log, how the log's own `[+Nms]` prefix and the telemetry `*_ms` fields relate.
///
/// They are two lazily-anchored `Instant`s: the log's starts at the first log line (near
/// DLL_PROCESS_ATTACH), the telemetry clock's at the first `boot_view_epoch_ms()` call (once the
/// boot view runs, seconds later). The difference is a constant for the process -- but nothing
/// emitted it, so every session that wanted to line a log line up against an oracle re-derived it by
/// pairing a stamp against the line that wrote it, by hand, from scratch. The DLL holds both
/// numbers; one line ends the re-derivation for good.
///
/// Called from the per-Present exposure recorder purely because that is somewhere that already runs
/// early and already reads the boot-view clock. Steady-state cost is one atomic load.
pub fn log_clock_map_once() {
    use er_telemetry_core::counters::{LOG_EPOCH_OFFSET_LOGGED, LOG_EPOCH_OFFSET_MS};
    if LOG_EPOCH_OFFSET_LOGGED.load(Ordering::SeqCst) != 0 {
        return;
    }
    if LOG_EPOCH_OFFSET_LOGGED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Some(telemetry_ms) = boot_view_epoch_ms_if_anchored() else {
        // The telemetry clock has not started, so there is no offset to state yet. Re-arm rather
        // than reporting a made-up one; the next frame tries again.
        LOG_EPOCH_OFFSET_LOGGED.store(0, Ordering::SeqCst);
        return;
    };
    let log_ms = process_log_elapsed_ms() as u64;
    // Bracket the reading. The offset is the difference between two clocks read a few instructions
    // apart, so anything that delays the thread between them -- a Wine scheduler preemption is the
    // realistic one -- inflates it by exactly that delay, and the result would be trusted. Read the
    // telemetry clock again afterwards: if it moved, the two readings were not simultaneous and the
    // difference is not the epoch gap. Re-arm and let a later frame produce a clean pair rather than
    // publishing a number that is off by an unknown amount.
    let telemetry_after = boot_view_epoch_ms_if_anchored().unwrap_or(u64::MAX);
    if telemetry_after.saturating_sub(telemetry_ms) > CLOCK_MAP_MAX_BRACKET_MS {
        LOG_EPOCH_OFFSET_LOGGED.store(0, Ordering::SeqCst);
        return;
    }
    let offset = log_ms.saturating_sub(telemetry_ms);
    LOG_EPOCH_OFFSET_MS.store(offset as usize, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "CLOCK MAP: this log's [+Nms] prefix and every telemetry *_ms field are DIFFERENT epochs. Read back to back just now: log=+{log_ms}ms telemetry={telemetry_ms}ms, so offset = log - telemetry = {offset}ms, constant for this process. To convert: log_ms = telemetry_ms + {offset}; telemetry_ms = log_ms - {offset}. (log epoch = first log line, near DLL_PROCESS_ATTACH; telemetry epoch = first boot-view tick. Also emitted as oracle_log_epoch_offset_ms.)"
    ));
}

/// Emit the post-release watch oracles plus the boot-view null detector. Called from the telemetry
/// writer.
pub fn cover_after_release_write(body: &mut String) {
    use er_telemetry_core::counters::{
        BOOT_VIEW_DRAW_AFTER_STOP, BOOT_VIEW_DRAW_AFTER_STOP_FIRST_MS,
        BOOT_VIEW_DRAW_AFTER_STOP_TOTAL, BOOT_VIEW_FADE_HELD_MS, BOOT_VIEW_FADE_HOLD_HONORED,
        BOOT_VIEW_FADE_HOLD_REASSERTS, BOOT_VIEW_FADE_HOLD_REASSERTS_FIRST_MS,
        BOOT_VIEW_FADE_HOLD_REFUSED, BOOT_VIEW_NONFADE_DRAW_DURING_FADE,
        BOOT_VIEW_NONFADE_DRAW_DURING_FADE_FIRST_MS, BOOT_VIEW_STOP_MS,
        COVER_PLATE_AFTER_RELEASE_SAMPLES, COVER_PLATE_VISIBLE_AFTER_RELEASE,
        COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS, COVER_PLATE_VISIBLE_AFTER_RELEASE_LAST_MS,
        COVER_PLATE_VISIBLE_AFTER_RELEASE_MAX_RUN, IN_GAME_MENU_OPEN_EDGES,
        IN_GAME_MENU_OPEN_FIRST_MS, IN_GAME_MENU_OPEN_FIRST_MS_AFTER_COVER_STOP,
        IN_GAME_MENU_OPEN_LAST_MS, IN_GAME_MENU_OPEN_MS_FIRST_N, IN_GAME_MENU_OPEN_MS_FIRST_N_LEN,
        LOG_EPOCH_OFFSET_MS, NATIVE_LS_ACTIVITY_AFTER_RELEASE_FADEOUTS,
        NATIVE_LS_ACTIVITY_AFTER_RELEASE_FIRST_MS, NATIVE_LS_ACTIVITY_AFTER_RELEASE_UPDATES,
    };
    push_json_usize(
        body,
        "oracle_boot_view_draw_after_stop",
        BOOT_VIEW_DRAW_AFTER_STOP.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_draw_after_stop_total",
        BOOT_VIEW_DRAW_AFTER_STOP_TOTAL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_draw_after_stop_first_ms",
        BOOT_VIEW_DRAW_AFTER_STOP_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_stop_ms",
        BOOT_VIEW_STOP_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_cover_after_release_samples",
        COVER_PLATE_AFTER_RELEASE_SAMPLES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_cover_plate_visible_after_release",
        COVER_PLATE_VISIBLE_AFTER_RELEASE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_cover_plate_visible_after_release_first_ms",
        COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_cover_plate_visible_after_release_last_ms",
        COVER_PLATE_VISIBLE_AFTER_RELEASE_LAST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_cover_plate_visible_after_release_max_run",
        COVER_PLATE_VISIBLE_AFTER_RELEASE_MAX_RUN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_activity_after_release_updates",
        NATIVE_LS_ACTIVITY_AFTER_RELEASE_UPDATES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_activity_after_release_fadeouts",
        NATIVE_LS_ACTIVITY_AFTER_RELEASE_FADEOUTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_activity_after_release_first_ms",
        NATIVE_LS_ACTIVITY_AFTER_RELEASE_FIRST_MS.load(Ordering::SeqCst),
    );
    // The fade window (2026-08-22, second round). Everything above opens only once
    // `BOOT_VIEW_STOPPED` has latched, and the reported defect happens before that -- during the
    // release fade. These seven measure the fade window itself:
    //   oracle_boot_view_nonfade_draw_during_fade > 0
    //       the opaque cover path (which rasterizes the portrait) drew while the fade was running.
    //       That is the defect. Expected 0; `_first_ms` minus `oracle_boot_view_fade_start_ms` says
    //       how far into the fade it happened.
    //   oracle_boot_view_fade_hold_reasserts > 0
    //       the native-loading hold went true again mid-fade -- the pressure the fix exists to
    //       absorb. Split into `_refused` (an over-matched Scaleform "fadeout" label from another
    //       movie, e.g. the in-world menu opening; the fade carried on) and `_honored` (the game's
    //       own `CS::LoadingScreen::Update` genuinely ticked again, so the fade paused instead of
    //       completing over live loading art). Refused > 0 with nonfade_draw == 0 is the fix working.
    //   oracle_boot_view_fade_held_ms
    //       total ms honored holds paused the fade. Nonzero means a fade took longer in wall clock
    //       than `BOOT_VIEW_RELEASE_FADE_MS` while still showing the full ramp.
    push_json_usize(
        body,
        "oracle_boot_view_nonfade_draw_during_fade",
        BOOT_VIEW_NONFADE_DRAW_DURING_FADE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_nonfade_draw_during_fade_first_ms",
        BOOT_VIEW_NONFADE_DRAW_DURING_FADE_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_hold_reasserts",
        BOOT_VIEW_FADE_HOLD_REASSERTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_hold_reasserts_first_ms",
        BOOT_VIEW_FADE_HOLD_REASSERTS_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_hold_refused",
        BOOT_VIEW_FADE_HOLD_REFUSED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_hold_honored",
        BOOT_VIEW_FADE_HOLD_HONORED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_held_ms",
        BOOT_VIEW_FADE_HELD_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_in_game_menu_open_edges",
        IN_GAME_MENU_OPEN_EDGES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_in_game_menu_open_last_ms",
        IN_GAME_MENU_OPEN_LAST_MS.load(Ordering::SeqCst),
    );
    // The one to read next to `oracle_cover_plate_visible_after_release_first_ms`: same clock, so
    // the interval between the user's press and the cover coming back is a subtraction.
    push_json_usize(
        body,
        "oracle_in_game_menu_open_first_ms_after_cover_stop",
        IN_GAME_MENU_OPEN_FIRST_MS_AFTER_COVER_STOP.load(Ordering::SeqCst),
    );
    // Every edge, not just the ones past a cover stop. A press that lands during the release fade
    // -- the one the report is about -- has no entry in the after-stop latch above, and edge #1's
    // timestamp used to be unrecoverable from telemetry for exactly that reason. A 0 at an index
    // below `_edges` means that edge fired before the boot-view clock was anchored.
    push_json_usize(
        body,
        "oracle_in_game_menu_open_first_ms",
        IN_GAME_MENU_OPEN_FIRST_MS.load(Ordering::SeqCst),
    );
    let menu_edges = (0..IN_GAME_MENU_OPEN_MS_FIRST_N_LEN)
        .map(|i| {
            IN_GAME_MENU_OPEN_MS_FIRST_N[i]
                .load(Ordering::SeqCst)
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(",");
    body.push_str(&format!(
        "  \"oracle_in_game_menu_open_ms_first_n\": [{menu_edges}],\n"
    ));
    // Add to any `*_ms` above to reach the matching `[+Nms]` DLL-log prefix; subtract to go back.
    push_json_usize(
        body,
        "oracle_log_epoch_offset_ms",
        LOG_EPOCH_OFFSET_MS.load(Ordering::SeqCst),
    );
}
