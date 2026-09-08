//! Per-load-window portrait verdict semaphores.
//!
//! Moved verbatim from er-quickload `telemetry/portrait_load_windows.rs` with the loading-cover
//! crate extraction. The product functions it reads -- the debug-log sink, the JSON emitter and the
//! boot-view clock -- cross the seam through [`crate::host`] and [`crate::loading_cover_host`];
//! every call site keeps its original name.

use std::sync::Mutex;

use crate::host::{append_autoload_debug, boot_view_epoch_ms};
use crate::loading_cover_host::push_json_usize;
use crate::portrait_semaphores::LS_PORTRAIT_SMALL_MAX_SIDE;
use std::sync::atomic::{AtomicUsize, Ordering};

// Per-load-window portrait verdict SEMAPHORES (user directive 2026-07-31: "get the semaphores to
// show us that our render portrait isn't working on every load").
//
// Telemetry-only: this file reads existing counters and native-hook timestamps; it changes no
// product behavior. For every native loading-screen window in a session (boot, profile switches,
// deaths, fast travel -- anything that ticks `CS::LoadingScreen::Update`), it latches one verdict
// record: did the custom rendered portrait actually display during that window, at what size, and
// if not, which pipeline stage was empty. The per-window verdict is exposed three ways:
//   1. a `PORTRAIT-LOADWIN VERDICT #n` debug-log line at window close (human-readable per load),
//   2. scalar aggregate oracles (`oracle_portrait_loadwin_total/ok_full/...`),
//   3. `oracle_portrait_loadwin_history` -- a JSON array of the last N window records, so one
//      telemetry snapshot names every load of the session and its failure class.
//
// Window boundaries: Open when `LOADING_SCREEN_UPDATE_LAST_MS` turns fresh (the native
// LoadingScreen update hook stamps it every frame the screen is live); Closed after it has been
// quiet for `LOADWIN_QUIET_CLOSE_MS`. Sampled from the telemetry writer (~4 Hz), which bounds
// boundary error to ~250 ms on windows that last seconds -- fine for verdict purposes.
//
// Verdict classes (`cause`):
//   ok-full                 displayed, published side >= LOADWIN_FULL_MIN_SIDE (the product bar)
//   displayed-small         displayed, published side <= the small threshold (the PIXELATED defect)
//   displayed-mid           displayed, published side in between
//   displayed-stale         overlay drew frames but nothing published this window: the shown head
//                           is a prior window's held frame (the WRONG/STALE-portrait defect)
//   published-not-displayed publishes happened but the overlay never drew them
//   captures-rejected       captures arrived but every publish was rejected (neutral/small gate)
//   kicked-no-publish       a renderer build was kicked but no capture ever published
//   no-kick                 the pipeline never even targeted this window
// A `+short` suffix marks windows shorter than the ~confirm->publish latency, where a missing
// portrait is expected-by-latency rather than a pipeline failure.
// A `+not-ours` suffix marks a miss on a loading screen the product's cover never owned -- a fast
// travel, a death respawn, an area transition. Those are excluded from the miss aggregates and
// counted in `oracle_portrait_loadwin_not_ours` instead; `cover_owned=` on the verdict line carries
// the raw answer for every window. Without it, run br-20260831-160354-2513 scored 2 of its 4
// windows as portrait failures when both were fast travels and both real load windows were ok-full.

/// Published-vs-loaded identity check for one closed window (bd er-effects-rs-qoqc defect 6 /
/// er-effects-rs-91zb). Implemented but UNPROVEN -- it changes no rendering, but nothing has yet
/// observed it fire on a live run.
///
/// Every pre-existing portrait oracle compared our target slot against the currently-resident
/// character. That is silent about the failure the user actually sees: on 2026-08-02 the pipeline
/// published slot 9's head and displayed it for 29.7 seconds while slot 5 loaded, and the window
/// still closed `cause=ok-full`. This compares the two things the loading screen's pixels really
/// depend on -- the slot we published, and the slot whose deserialize completed -- plus their
/// record name hashes as an independent second axis (a slot index can coincide; a name hash is the
/// character). Returns a short tag for the verdict line.
fn portrait_published_identity_check(published_slot_tag: usize) -> &'static str {
    use crate::portrait_identity::{PublishedIdentity, published_identity_verdict};
    use er_telemetry_core::counters::{
        LS_PORTRAIT_PUBLISHED_NAME_HASH, PORTRAIT_PUBLISHED_IDENTITY_CHECKS,
        PORTRAIT_PUBLISHED_NAME_HASH_MISMATCHES, PORTRAIT_PUBLISHED_SLOT_MISMATCHES,
        PORTRAIT_TARGET_NAME_HASH, SYSTEM_QUIT_FRESH_DESER_DONE_SLOT,
    };
    // Ground truth for "which character loaded" is the slot whose deserialize completed, not
    // `GameMan.save_slot`: the game's own selector and our own `set_save_slot` both write ac0 for
    // reasons unrelated to that question, which is exactly how the 29.7s window went unreported.
    let loaded = SYSTEM_QUIT_FRESH_DESER_DONE_SLOT
        .load(Ordering::SeqCst)
        .checked_sub(1)
        .map(|s| s as i32);
    let verdict = published_identity_verdict(published_slot_tag, loaded);
    match verdict {
        PublishedIdentity::NothingPublished => "no-publish",
        PublishedIdentity::NoLoadedSlot => "no-loaded-slot",
        PublishedIdentity::Match => {
            PORTRAIT_PUBLISHED_IDENTITY_CHECKS.fetch_add(1, Ordering::SeqCst);
            // Second axis: the published head's record name hash against the target the build kick
            // stamped. A slot index can match while the record underneath it was rewritten.
            let published_hash = LS_PORTRAIT_PUBLISHED_NAME_HASH.load(Ordering::SeqCst);
            let target_hash = PORTRAIT_TARGET_NAME_HASH.load(Ordering::SeqCst);
            if published_hash != 0 && target_hash != 0 && published_hash != target_hash {
                let n = PORTRAIT_PUBLISHED_NAME_HASH_MISMATCHES.fetch_add(1, Ordering::SeqCst) + 1;
                append_autoload_debug(format_args!(
                    "PORTRAIT-PUBLISHED-IDENTITY NAME MISMATCH #{n}: published head name hash 0x{published_hash:x} != loaded character's record hash 0x{target_hash:x} (slot matched) -- the displayed face is not the loaded character"
                ));
                "name-mismatch"
            } else {
                "ok"
            }
        }
        PublishedIdentity::SlotMismatch { published, loaded } => {
            PORTRAIT_PUBLISHED_IDENTITY_CHECKS.fetch_add(1, Ordering::SeqCst);
            let n = PORTRAIT_PUBLISHED_SLOT_MISMATCHES.fetch_add(1, Ordering::SeqCst) + 1;
            append_autoload_debug(format_args!(
                "PORTRAIT-PUBLISHED-IDENTITY SLOT MISMATCH #{n}: the loading screen displayed slot {published}'s portrait for this whole window while slot {loaded} was the character that loaded"
            ));
            "slot-mismatch"
        }
    }
}

/// Window considered closed after the native LoadingScreen update has been quiet this long.
const LOADWIN_QUIET_CLOSE_MS: usize = 1500;
/// Backstop for the deferred window-state release: if the boot-view cover has still not latched
/// stopped this long after a window closed, release anyway.
///
/// The measured gap it has to cover is ~710 ms (run br-20260822-184123-fa3d: native LoadingScreen
/// last ticked at 35504 ms so the window closes at 37004 ms, and the cover's release fade completed
/// at 37714 ms). This is that gap with a large margin, and it exists only so a session where the
/// cover never arms at all cannot leave the release pending forever.
const LOADWIN_RESET_COVER_GRACE_MS: usize = 5000;
/// Published side at/above this = the full-size product portrait.
const LOADWIN_FULL_MIN_SIDE: usize = 1024;
/// Windows shorter than this get the `+short` suffix (below the measured ~2.3s confirm->publish
/// latency, so "no portrait" is latency, not a broken pipeline).
const LOADWIN_SHORT_WINDOW_MS: usize = 2500;
/// History ring capacity (records, newest last).
const LOADWIN_HISTORY_CAP: usize = 16;

const LOADWIN_CLOSED: usize = 0;
const LOADWIN_OPEN: usize = 1;

static LOADWIN_STATE: AtomicUsize = AtomicUsize::new(LOADWIN_CLOSED);
static LOADWIN_INDEX: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_OPEN_MS: AtomicUsize = AtomicUsize::new(0);
// Cumulative-counter baselines snapshotted at window open.
static LOADWIN_BASE_ONTO_DRAWS: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_BASE_RGBA_VERSION: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_BASE_REJECTS: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_BASE_TABLE_BUILDS: AtomicUsize = AtomicUsize::new(0);
/// Native-loading-screen exposure frames at window open, so each window reports its own
/// flash-through count (`expo` in the history record) rather than the session total.
static LOADWIN_BASE_EXPOSURE: AtomicUsize = AtomicUsize::new(0);
// Maxima tracked while the window is open.
static LOADWIN_MAX_CAP_SIDE: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_MAX_PUB_SIDE: AtomicUsize = AtomicUsize::new(0);
/// 1 once a window has closed and its portrait-state release has not yet been able to run. The
/// release cannot happen at the close itself -- see `portrait_loadwin_try_release_window_state`.
static LOADWIN_RESET_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms the pending release was armed, for the grace backstop.
static LOADWIN_RESET_PENDING_MS: AtomicUsize = AtomicUsize::new(0);
// Last update-hook timestamp observed while the window was open. The close path must use this,
// not the live counter: boot-view resets LOADING_SCREEN_UPDATE_LAST_MS to 0 at its window
// teardown, which made window #1 close with dur_ms=0 (run 20260731-090100).
static LOADWIN_LAST_LIVE_MS: AtomicUsize = AtomicUsize::new(0);
// Aggregates across the session.
static LOADWIN_TOTAL: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_OK_FULL: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_DISPLAYED_SMALL: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_DISPLAYED_STALE: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_MISSING: AtomicUsize = AtomicUsize::new(0);
/// Windows the product's cover does not own at all -- a fast travel, a death respawn, an area
/// transition. Counted apart from [`LOADWIN_MISSING`] because they are not misses.
static LOADWIN_NOT_OURS: AtomicUsize = AtomicUsize::new(0);
/// 1 if the loading cover owned this window's loading screen, sampled while the window is still
/// open and latched sticky.
///
/// Sampled at open, never at close, and the run that forces it is br-20260831-160354-2513.
/// `cover_owns_current_loading_screen()` goes false once the cover stops and stamps its load
/// witness -- and the cover stops before the window closes on every healthy load (the window closes
/// `LOADWIN_QUIET_CLOSE_MS` after the last native tick, the cover lets go before that). So asking at
/// close would report the user's own System->Quit->Load Character reload as "not ours": in that run
/// the cover stopped at 239858 ms and window #3 closed at 241344 ms. Asking at open gives the four
/// windows of that run their true answers -- boot ours, fast travel not, reload ours, fast travel
/// not.
static LOADWIN_COVER_OWNED: AtomicUsize = AtomicUsize::new(0);
/// Last N per-window records, each a pre-rendered JSON object string (ASCII only).
static LOADWIN_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Advance the window state machine and emit the loadwin oracles. Called from the telemetry
/// writer on every write (~4 Hz). Read-only against game state; writes only our own counters.
pub fn portrait_loadwin_sample_and_write(body: &mut String) {
    portrait_loadwin_tick();
    push_json_usize(
        body,
        "oracle_portrait_loadwin_total",
        LOADWIN_TOTAL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_loadwin_ok_full",
        LOADWIN_OK_FULL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_loadwin_displayed_small",
        LOADWIN_DISPLAYED_SMALL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_loadwin_displayed_stale",
        LOADWIN_DISPLAYED_STALE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_loadwin_missing",
        LOADWIN_MISSING.load(Ordering::SeqCst),
    );
    // Windows the cover never owned (fast travel / death / area transition). Reported separately so
    // an acceptance gate on `_missing` stops failing on a warp -- see the close path for the run.
    push_json_usize(
        body,
        "oracle_portrait_loadwin_not_ours",
        LOADWIN_NOT_OURS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_loadwin_open",
        LOADWIN_STATE.load(Ordering::SeqCst),
    );
    let history = match LOADWIN_HISTORY.lock() {
        Ok(g) => g.join(","),
        Err(p) => p.into_inner().join(","),
    };
    body.push_str(&format!(
        "  \"oracle_portrait_loadwin_history\": [{history}],\n"
    ));
}

fn loadwin_max_side_packed(packed: usize) -> usize {
    ((packed >> 16) & 0xffff).max(packed & 0xffff)
}

fn portrait_loadwin_tick() {
    use er_telemetry_core::counters::{
        LOADING_BG_PORTRAIT_DIMS, LOADING_BG_PORTRAIT_RGBA_VERSION, LOADING_SCREEN_UPDATE_LAST_MS,
        LS_PORTRAIT_LAST_H, LS_PORTRAIT_LAST_W, LS_PORTRAIT_REJECTED_PUBLISHES,
        PORTRAIT_ONTO_DRAW_HITS, PROFILE_LOADSCREEN_TABLE_BUILDS,
    };
    let now_ms = boot_view_epoch_ms().max(1) as usize;
    let update_last = LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst);
    let screen_live =
        update_last != 0 && now_ms.saturating_sub(update_last) < LOADWIN_QUIET_CLOSE_MS;
    let state = LOADWIN_STATE.load(Ordering::SeqCst);
    if state == LOADWIN_CLOSED && screen_live {
        // OPEN: snapshot the cumulative baselines this window's deltas are measured against.
        LOADWIN_OPEN_MS.store(now_ms, Ordering::SeqCst);
        LOADWIN_BASE_ONTO_DRAWS.store(
            PORTRAIT_ONTO_DRAW_HITS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        LOADWIN_BASE_RGBA_VERSION.store(
            LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        LOADWIN_BASE_REJECTS.store(
            LS_PORTRAIT_REJECTED_PUBLISHES.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        LOADWIN_BASE_TABLE_BUILDS.store(
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        LOADWIN_BASE_EXPOSURE.store(
            er_telemetry_core::counters::NATIVE_LS_EXPOSURE_FRAMES.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        // Ownership, asked here and only here -- see LOADWIN_COVER_OWNED for why close is too late.
        LOADWIN_COVER_OWNED.store(
            er_telemetry_core::counters::cover_owns_current_loading_screen() as usize,
            Ordering::SeqCst,
        );
        // Reject-attribution baseline for windows that are not profile switches
        // (er-effects-rs-k979). `loading_portrait_window_reset_for_switch` -- which sets it at the
        // exact instant a switch arms -- has only one caller, the own-menu switch arm. Boot is fine
        // because the baseline starts at 0, but a death or fast-travel window would inherit the
        // stale baseline from the last switch and misfile its warm-up reject as a post-publish
        // fault: the same false failure this work removes, on a different path.
        //
        // Setting it here closes that to a bounded race rather than a permanent error. This runs
        // from the ~4 Hz telemetry writer, so a reject in the first ~250 ms of a non-switch window
        // can still be misfiled; switch windows keep the exact arm-time value set above it. Neither
        // path has been exercised against a live reject yet.
        er_telemetry_core::counters::LS_PORTRAIT_REJECT_PUBLISH_BASELINE.store(
            LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        LOADWIN_MAX_CAP_SIDE.store(0, Ordering::SeqCst);
        LOADWIN_MAX_PUB_SIDE.store(0, Ordering::SeqCst);
        LOADWIN_LAST_LIVE_MS.store(update_last, Ordering::SeqCst);
        // A new window supersedes any deferred release from the previous one: this window will arm
        // its own at close. Without this, a release deferred behind a character switch would fire
        // in the middle of the switch's next window.
        LOADWIN_RESET_PENDING.store(0, Ordering::SeqCst);
        LOADWIN_STATE.store(LOADWIN_OPEN, Ordering::SeqCst);
        let i = LOADWIN_INDEX.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "portrait-loadwin: OPEN #{i} at {now_ms}ms (native LoadingScreen updating)"
        ));
        return;
    }
    if state == LOADWIN_OPEN {
        if update_last != 0 {
            LOADWIN_LAST_LIVE_MS.fetch_max(update_last, Ordering::SeqCst);
        }
        // Track capture/publish size maxima while the window is up. Publishes are the frames the
        // overlay can actually show; captures prove what the readback saw even if publish failed.
        // Only sample the sticky last-capture dims once a capture event (publish or reject) has
        // happened this window -- otherwise the prior window's last dims bleed into this one
        // (window #2 of run 20260731-090100 reported cap_side=1542 from the boot window's residue).
        let cap_events_this_window = LOADING_BG_PORTRAIT_RGBA_VERSION
            .load(Ordering::SeqCst)
            .saturating_sub(LOADWIN_BASE_RGBA_VERSION.load(Ordering::SeqCst))
            + LS_PORTRAIT_REJECTED_PUBLISHES
                .load(Ordering::SeqCst)
                .saturating_sub(LOADWIN_BASE_REJECTS.load(Ordering::SeqCst));
        if cap_events_this_window > 0 {
            let cap_side = LS_PORTRAIT_LAST_W
                .load(Ordering::SeqCst)
                .max(LS_PORTRAIT_LAST_H.load(Ordering::SeqCst));
            LOADWIN_MAX_CAP_SIDE.fetch_max(cap_side, Ordering::SeqCst);
        }
        // Only count published dims once a publish happened this window; DIMS itself is sticky
        // from prior windows.
        if LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst)
            > LOADWIN_BASE_RGBA_VERSION.load(Ordering::SeqCst)
        {
            LOADWIN_MAX_PUB_SIDE.fetch_max(
                loadwin_max_side_packed(LOADING_BG_PORTRAIT_DIMS.load(Ordering::SeqCst)),
                Ordering::SeqCst,
            );
        }
        // Sticky: a window that was ours at any open sample stays ours, so a cover that armed a tick
        // late cannot downgrade its own load to "not ours".
        if er_telemetry_core::counters::cover_owns_current_loading_screen() {
            LOADWIN_COVER_OWNED.store(1, Ordering::SeqCst);
        }
        if !screen_live {
            portrait_loadwin_close(
                LOADWIN_LAST_LIVE_MS
                    .load(Ordering::SeqCst)
                    .max(LOADWIN_OPEN_MS.load(Ordering::SeqCst)),
            );
        }
    }
    // Deferred portrait-state release. Runs on the closing tick and on every later tick until its
    // preconditions hold; skipped entirely while a window is open.
    if LOADWIN_STATE.load(Ordering::SeqCst) == LOADWIN_CLOSED {
        portrait_loadwin_try_release_window_state(now_ms);
    }
}

fn portrait_loadwin_close(close_ms: usize) {
    use er_telemetry_core::counters::{
        LOADING_BG_PORTRAIT_RGBA_VERSION, LS_PORTRAIT_PUBLISHED_SLOT,
        LS_PORTRAIT_REJECTED_PUBLISHES, PORTRAIT_KICK_SLOT_KEY, PORTRAIT_ONTO_DRAW_HITS,
        PROFILE_LOADSCREEN_TABLE_BUILDS,
    };
    LOADWIN_STATE.store(LOADWIN_CLOSED, Ordering::SeqCst);
    let i = LOADWIN_INDEX.load(Ordering::SeqCst);
    let open_ms = LOADWIN_OPEN_MS.load(Ordering::SeqCst);
    let dur_ms = close_ms.saturating_sub(open_ms);
    let displayed = PORTRAIT_ONTO_DRAW_HITS
        .load(Ordering::SeqCst)
        .saturating_sub(LOADWIN_BASE_ONTO_DRAWS.load(Ordering::SeqCst));
    let publishes = LOADING_BG_PORTRAIT_RGBA_VERSION
        .load(Ordering::SeqCst)
        .saturating_sub(LOADWIN_BASE_RGBA_VERSION.load(Ordering::SeqCst));
    let rejects = LS_PORTRAIT_REJECTED_PUBLISHES
        .load(Ordering::SeqCst)
        .saturating_sub(LOADWIN_BASE_REJECTS.load(Ordering::SeqCst));
    let kicked = PROFILE_LOADSCREEN_TABLE_BUILDS
        .load(Ordering::SeqCst)
        .saturating_sub(LOADWIN_BASE_TABLE_BUILDS.load(Ordering::SeqCst))
        > 0
        || PORTRAIT_KICK_SLOT_KEY.load(Ordering::SeqCst) != 0;
    let cap_side = LOADWIN_MAX_CAP_SIDE.load(Ordering::SeqCst);
    let pub_side = LOADWIN_MAX_PUB_SIDE.load(Ordering::SeqCst);
    let slot_tag = LS_PORTRAIT_PUBLISHED_SLOT.load(Ordering::SeqCst);
    let identity = portrait_published_identity_check(slot_tag);
    // Frames of this window where the game's own loading screen reached the screen uncovered
    // (er-effects-rs-wmw defect #1). Orthogonal to the portrait verdict: a window can publish a
    // perfect 1542 head and still have flashed vanilla for a frame.
    let exposure = er_telemetry_core::counters::NATIVE_LS_EXPOSURE_FRAMES
        .load(Ordering::SeqCst)
        .saturating_sub(LOADWIN_BASE_EXPOSURE.load(Ordering::SeqCst));
    let base_cause = if displayed > 0 {
        if publishes == 0 {
            "displayed-stale"
        } else if pub_side >= LOADWIN_FULL_MIN_SIDE {
            "ok-full"
        } else if pub_side != 0 && pub_side as u32 <= LS_PORTRAIT_SMALL_MAX_SIDE {
            "displayed-small"
        } else {
            "displayed-mid"
        }
    } else if publishes > 0 {
        "published-not-displayed"
    } else if rejects > 0 {
        "captures-rejected"
    } else if kicked {
        "kicked-no-publish"
    } else {
        "no-kick"
    };
    let short = dur_ms < LOADWIN_SHORT_WINDOW_MS;
    // Whose loading screen was this? Without this, a window the product never covers is scored as a
    // portrait failure, and run br-20260831-160354-2513 scored two of them: window #2
    // `published-not-displayed` and window #4 `kicked-no-publish` were both plain fast travels
    // (`gm-snap: ... warp_requested=true` at log 2657 and 8479). Nothing was wrong in either. The
    // cover had released normally at 33747 ms (`reason=1`) and a warp is the game's own screen, so
    // the composite never ran and the pump's in-world epoch fast-path
    // (`lookat_bone_hooks.rs` `BOOT_VIEW_EPOCH_WORLD_LIVE == cur`) correctly stopped the capture.
    // Both classes then read as defects: 2 of 4 windows "failed" on a session whose only real load
    // windows, #1 and #3, were both `ok-full`. Same distinction, same predicate and same argument as
    // `NATIVE_LS_GATE_UNOWNED_LOAD` on the exposure side.
    let owned = LOADWIN_COVER_OWNED.load(Ordering::SeqCst) != 0;
    let displayed_ok = matches!(base_cause, "ok-full" | "displayed-small" | "displayed-mid");
    // Only a miss gets excused. A not-ours window that somehow displayed a portrait is still
    // reported as what it displayed -- suppressing that would hide a cover drawing where it should
    // not, which is the opposite defect and a worse one.
    let not_ours = !owned && !displayed_ok;
    let cause = format!(
        "{base_cause}{}{}",
        if short { "+short" } else { "" },
        if not_ours { "+not-ours" } else { "" }
    );
    LOADWIN_TOTAL.fetch_add(1, Ordering::SeqCst);
    if not_ours {
        LOADWIN_NOT_OURS.fetch_add(1, Ordering::SeqCst);
    } else {
        match base_cause {
            "ok-full" => {
                LOADWIN_OK_FULL.fetch_add(1, Ordering::SeqCst);
            }
            "displayed-small" => {
                LOADWIN_DISPLAYED_SMALL.fetch_add(1, Ordering::SeqCst);
            }
            "displayed-stale" => {
                LOADWIN_DISPLAYED_STALE.fetch_add(1, Ordering::SeqCst);
            }
            _ => {
                LOADWIN_MISSING.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    append_autoload_debug(format_args!(
        "PORTRAIT-LOADWIN VERDICT #{i}: cause={cause} cover_owned={} dur_ms={dur_ms} displayed={displayed} publishes={publishes} rejects={rejects} kicked={} cap_max_side={cap_side} pub_max_side={pub_side} slot_tag={slot_tag} identity={identity} native_exposure_frames={exposure} window={open_ms}..{close_ms}ms",
        owned as u8, kicked as u8
    ));
    let record = format!(
        "{{\"i\":{i},\"open_ms\":{open_ms},\"dur_ms\":{dur_ms},\"disp\":{displayed},\"pub\":{publishes},\"rej\":{rejects},\"kick\":{},\"owned\":{},\"cap_side\":{cap_side},\"pub_side\":{pub_side},\"slot\":{slot_tag},\"expo\":{exposure},\"cause\":\"{cause}\"}}",
        kicked as u8, owned as u8
    );
    let mut guard = match LOADWIN_HISTORY.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.len() >= LOADWIN_HISTORY_CAP {
        guard.remove(0);
    }
    guard.push(record);
    drop(guard);
    // Arm the portrait-state release. It cannot run here -- see
    // `portrait_loadwin_try_release_window_state` for why the close is too early.
    LOADWIN_RESET_PENDING.store(1, Ordering::SeqCst);
    // Stamped now, not with `close_ms`: `close_ms` is the window's last live native tick, so the
    // grace below would silently start `LOADWIN_QUIET_CLOSE_MS` in the past.
    LOADWIN_RESET_PENDING_MS.store(boot_view_epoch_ms().max(1) as usize, Ordering::SeqCst);
}

/// Release the portrait pipeline's per-window state once the loading window is over and nothing is
/// still displaying its head.
///
/// The dangling window (2026-08-22). `loading_portrait_window_reset` had zero callers in the whole
/// workspace; the only live reset was its switch variant, reached solely from
/// `rearm_boot_progress_for_own_menu_load` -- a System->Quit slot confirm. So a normal load never
/// released the published head, the frozen crop envelope, `LOADING_BG_PORTRAIT_SPARED_RENDERER` (a
/// live `CSMenuProfModelRend` retained forever and never delete-enqueued),
/// `PORTRAIT_ANIM_BOUND_RENDERER`, `PROFILE_RT_PIN`/`PROFILE_DEPTH_PIN`, or
/// `PORTRAIT_WINDOW_TARGET_SLOT`.
///
/// What it BUYS, honestly: a future spurious cover has no portrait left to put on screen, and the
/// per-session renderer leak stops. It does not stop the cover surface itself reappearing -- our
/// compositor is not what draws that (`cover_after_release.rs` is the instrumentation that will
/// say what does).
///
/// Why it is deferred rather than done at the close. The window closes `LOADWIN_QUIET_CLOSE_MS`
/// after the native loading screen stops ticking, and our cover outlives that by its release fade.
/// In run br-20260822-184123-fa3d the native screen last ticked at 35504 ms, so the window closes
/// at 37004 ms, while the cover's release fade did not complete until 37714 ms -- and
/// `portrait_onto_draw_hits` advanced by 10 across exactly that stretch. Releasing at the close
/// would therefore have blanked the portrait for the last ~710 ms of a fade that was still on
/// screen. So the close only arms the release; this runs it on a later ~4 Hz tick, once
/// `BOOT_VIEW_STOPPED` says the cover has genuinely let go.
///
/// Make-before-break is preserved by two guards:
///   * a System->Quit switch in flight (`BOOT_VIEW_OWN_MENU_LOAD_ACTIVE`) is skipped outright. That
///     latch is set at the switch rearm and cleared only when the switch's cover stops, so it spans
///     the return-to-title teardown window -- whose close would otherwise wipe state the
///     character-load window that follows is about to need. The pending flag is dropped when that
///     next window opens and re-armed at its close, so nothing is lost by waiting.
///   * inside the reset itself, an outstanding provisional same-identity hold keeps the bridge and
///     the frozen crop envelope, exactly as the switch variant does -- so the head a confirm-press
///     chose to keep survives this path too.
///
/// Order vs the verdict. The reset clears counters `PORTRAIT-LOADWIN VERDICT` reads --
/// `LS_PORTRAIT_PUBLISHED_SLOT` (its `slot_tag` and its whole `identity=` check),
/// `PORTRAIT_TARGET_NAME_HASH`, and `PORTRAIT_KICK_SLOT_KEY` (`kicked=`). Deferring past the close
/// puts it strictly after the verdict line and the history record, so those numbers still describe
/// the window they name.
fn portrait_loadwin_try_release_window_state(now_ms: usize) {
    if LOADWIN_RESET_PENDING.load(Ordering::SeqCst) == 0 {
        return;
    }
    // A switch still in flight waits, and is not subject to the grace backstop below: the gap
    // between a switch's teardown window and its character-load window can legitimately exceed any
    // grace period, and firing inside it is the one outcome this must never produce.
    if er_telemetry_core::counters::BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0 {
        return;
    }
    let cover_released = er_telemetry_core::counters::BOOT_VIEW_STOPPED.load(Ordering::SeqCst) != 0;
    let waited_ms = now_ms.saturating_sub(LOADWIN_RESET_PENDING_MS.load(Ordering::SeqCst));
    // The backstop covers only the case where the cover never armed at all, so its stop latch will
    // never be set and the release would otherwise stay pending for the life of the process.
    if !cover_released && waited_ms < LOADWIN_RESET_COVER_GRACE_MS {
        return;
    }
    LOADWIN_RESET_PENDING.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "portrait-loadwin: releasing portrait window state at {now_ms}ms ({waited_ms}ms after the window closed, cover_released={}) -- dropping the published head, crop envelope, pins, anim binding and spared renderer so nothing survives into gameplay",
        cover_released as u8
    ));
    // Telemetry-writer/game-task thread, not the render thread. The reset bounded-drains in-flight
    // portrait jobs for up to ~15 ms; once per loading window, and the same drain the switch rearm
    // already performs from the confirm-press hook.
    crate::portrait_shared::loading_portrait_window_reset("loadwin-close");
}
