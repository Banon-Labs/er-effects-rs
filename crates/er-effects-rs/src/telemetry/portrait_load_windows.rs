// PER-LOAD-WINDOW PORTRAIT VERDICT SEMAPHORES (user directive 2026-07-31: "get the semaphores to
// show us that our render portrait isn't working on every load").
//
// Telemetry-only: this file reads existing counters and native-hook timestamps; it changes NO
// product behavior. For EVERY native loading-screen window in a session (boot, profile switches,
// deaths, fast travel -- anything that ticks `CS::LoadingScreen::Update`), it latches one verdict
// record: did the custom rendered portrait actually DISPLAY during that window, at what size, and
// if not, which pipeline stage was empty. The per-window verdict is exposed three ways:
//   1. a `PORTRAIT-LOADWIN VERDICT #n` debug-log line at window close (human-readable per load),
//   2. scalar aggregate oracles (`oracle_portrait_loadwin_total/ok_full/...`),
//   3. `oracle_portrait_loadwin_history` -- a JSON array of the last N window records, so one
//      telemetry snapshot names every load of the session and its failure class.
//
// Window boundaries: OPEN when `LOADING_SCREEN_UPDATE_LAST_MS` turns fresh (the native
// LoadingScreen update hook stamps it every frame the screen is live); CLOSED after it has been
// quiet for `LOADWIN_QUIET_CLOSE_MS`. Sampled from the telemetry writer (~4 Hz), which bounds
// boundary error to ~250 ms on windows that last seconds -- fine for verdict purposes.
//
// Verdict classes (`cause`):
//   ok-full                 displayed, published side >= LOADWIN_FULL_MIN_SIDE (the product bar)
//   displayed-small         displayed, published side <= the small threshold (the PIXELATED defect)
//   displayed-mid           displayed, published side in between
//   displayed-stale         overlay drew frames but NOTHING published this window: the shown head
//                           is a prior window's held frame (the WRONG/STALE-portrait defect)
//   published-not-displayed publishes happened but the overlay never drew them
//   captures-rejected       captures arrived but every publish was rejected (neutral/small gate)
//   kicked-no-publish       a renderer build was kicked but no capture ever published
//   no-kick                 the pipeline never even targeted this window
// A `+short` suffix marks windows shorter than the ~confirm->publish latency, where a missing
// portrait is expected-by-latency rather than a pipeline failure.

/// PUBLISHED-vs-LOADED identity check for one closed window (bd er-effects-rs-qoqc defect 6 /
/// er-effects-rs-91zb). IMPLEMENTED BUT UNPROVEN -- it changes no rendering, but nothing has yet
/// observed it fire on a live run.
///
/// Every pre-existing portrait oracle compared our TARGET slot against the currently-resident
/// character. That is silent about the failure the user actually sees: on 2026-08-02 the pipeline
/// published slot 9's head and displayed it for 29.7 seconds while slot 5 loaded, and the window
/// still closed `cause=ok-full`. This compares the two things the loading screen's pixels really
/// depend on -- the slot we PUBLISHED, and the slot whose deserialize COMPLETED -- plus their
/// record name hashes as an independent second axis (a slot index can coincide; a name hash is the
/// character). Returns a short tag for the verdict line.
fn portrait_published_identity_check(published_slot_tag: usize) -> &'static str {
    use er_loading_portrait::portrait_identity::{PublishedIdentity, published_identity_verdict};
    use er_telemetry::counters::{
        LS_PORTRAIT_PUBLISHED_NAME_HASH, PORTRAIT_PUBLISHED_IDENTITY_CHECKS,
        PORTRAIT_PUBLISHED_NAME_HASH_MISMATCHES, PORTRAIT_PUBLISHED_SLOT_MISMATCHES,
        PORTRAIT_TARGET_NAME_HASH, SYSTEM_QUIT_FRESH_DESER_DONE_SLOT,
    };
    // Ground truth for "which character loaded" is the slot whose deserialize completed, NOT
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

/// Window considered CLOSED after the native LoadingScreen update has been quiet this long.
const LOADWIN_QUIET_CLOSE_MS: usize = 1500;
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
/// Native-loading-screen exposure frames at window open, so each window reports its OWN
/// flash-through count (`expo` in the history record) rather than the session total.
static LOADWIN_BASE_EXPOSURE: AtomicUsize = AtomicUsize::new(0);
// Maxima tracked while the window is open.
static LOADWIN_MAX_CAP_SIDE: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_MAX_PUB_SIDE: AtomicUsize = AtomicUsize::new(0);
// Last update-hook timestamp observed while the window was open. The close path must use THIS,
// not the live counter: boot-view resets LOADING_SCREEN_UPDATE_LAST_MS to 0 at its window
// teardown, which made window #1 close with dur_ms=0 (run 20260731-090100).
static LOADWIN_LAST_LIVE_MS: AtomicUsize = AtomicUsize::new(0);
// Aggregates across the session.
static LOADWIN_TOTAL: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_OK_FULL: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_DISPLAYED_SMALL: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_DISPLAYED_STALE: AtomicUsize = AtomicUsize::new(0);
static LOADWIN_MISSING: AtomicUsize = AtomicUsize::new(0);
/// Last N per-window records, each a pre-rendered JSON object string (ASCII only).
static LOADWIN_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Advance the window state machine and emit the loadwin oracles. Called from the telemetry
/// writer on every write (~4 Hz). Read-only against game state; writes only our own counters.
pub(crate) fn portrait_loadwin_sample_and_write(body: &mut String) {
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
    use er_telemetry::counters::{
        LOADING_BG_PORTRAIT_DIMS, LOADING_BG_PORTRAIT_RGBA_VERSION, LOADING_SCREEN_UPDATE_LAST_MS,
        LS_PORTRAIT_LAST_H, LS_PORTRAIT_LAST_W, LS_PORTRAIT_REJECTED_PUBLISHES,
        PORTRAIT_ONTO_DRAW_HITS, PROFILE_LOADSCREEN_TABLE_BUILDS,
    };
    let now_ms = crate::experiments::boot_view_epoch_ms().max(1) as usize;
    let update_last = LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst);
    let screen_live = update_last != 0 && now_ms.saturating_sub(update_last) < LOADWIN_QUIET_CLOSE_MS;
    let state = LOADWIN_STATE.load(Ordering::SeqCst);
    if state == LOADWIN_CLOSED && screen_live {
        // OPEN: snapshot the cumulative baselines this window's deltas are measured against.
        LOADWIN_OPEN_MS.store(now_ms, Ordering::SeqCst);
        LOADWIN_BASE_ONTO_DRAWS.store(PORTRAIT_ONTO_DRAW_HITS.load(Ordering::SeqCst), Ordering::SeqCst);
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
            er_telemetry::counters::NATIVE_LS_EXPOSURE_FRAMES.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        // Reject-attribution baseline for windows that are NOT profile switches
        // (er-effects-rs-k979). `loading_portrait_window_reset_for_switch` -- which sets it at the
        // exact instant a switch arms -- has only ONE caller, the own-menu switch arm. Boot is fine
        // because the baseline starts at 0, but a death or fast-travel window would inherit the
        // STALE baseline from the last switch and misfile its warm-up reject as a post-publish
        // fault: the same false failure this work removes, on a different path.
        //
        // Setting it here closes that to a bounded race rather than a permanent error. This runs
        // from the ~4 Hz telemetry writer, so a reject in the first ~250 ms of a non-switch window
        // can still be misfiled; switch windows keep the exact arm-time value set above it. Neither
        // path has been exercised against a live reject yet.
        er_telemetry::counters::LS_PORTRAIT_REJECT_PUBLISH_BASELINE.store(
            LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        LOADWIN_MAX_CAP_SIDE.store(0, Ordering::SeqCst);
        LOADWIN_MAX_PUB_SIDE.store(0, Ordering::SeqCst);
        LOADWIN_LAST_LIVE_MS.store(update_last, Ordering::SeqCst);
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
        // Only sample the sticky last-capture dims once a capture EVENT (publish or reject) has
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
        // Only count published dims once a publish happened THIS window; DIMS itself is sticky
        // from prior windows.
        if LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst)
            > LOADWIN_BASE_RGBA_VERSION.load(Ordering::SeqCst)
        {
            LOADWIN_MAX_PUB_SIDE.fetch_max(
                loadwin_max_side_packed(LOADING_BG_PORTRAIT_DIMS.load(Ordering::SeqCst)),
                Ordering::SeqCst,
            );
        }
        if !screen_live {
            portrait_loadwin_close(
                LOADWIN_LAST_LIVE_MS
                    .load(Ordering::SeqCst)
                    .max(LOADWIN_OPEN_MS.load(Ordering::SeqCst)),
            );
        }
    }
}

fn portrait_loadwin_close(close_ms: usize) {
    use er_telemetry::counters::{
        LOADING_BG_PORTRAIT_RGBA_VERSION, LS_PORTRAIT_PUBLISHED_SLOT, LS_PORTRAIT_REJECTED_PUBLISHES,
        PORTRAIT_KICK_SLOT_KEY, PORTRAIT_ONTO_DRAW_HITS, PROFILE_LOADSCREEN_TABLE_BUILDS,
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
    // Frames of THIS window where the game's own loading screen reached the screen uncovered
    // (er-effects-rs-wmw defect #1). Orthogonal to the portrait verdict: a window can publish a
    // perfect 1542 head and still have flashed vanilla for a frame.
    let exposure = er_telemetry::counters::NATIVE_LS_EXPOSURE_FRAMES
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
    let cause = if short {
        format!("{base_cause}+short")
    } else {
        base_cause.to_string()
    };
    LOADWIN_TOTAL.fetch_add(1, Ordering::SeqCst);
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
    append_autoload_debug(format_args!(
        "PORTRAIT-LOADWIN VERDICT #{i}: cause={cause} dur_ms={dur_ms} displayed={displayed} publishes={publishes} rejects={rejects} kicked={} cap_max_side={cap_side} pub_max_side={pub_side} slot_tag={slot_tag} identity={identity} native_exposure_frames={exposure} window={open_ms}..{close_ms}ms",
        kicked as u8
    ));
    let record = format!(
        "{{\"i\":{i},\"open_ms\":{open_ms},\"dur_ms\":{dur_ms},\"disp\":{displayed},\"pub\":{publishes},\"rej\":{rejects},\"kick\":{},\"cap_side\":{cap_side},\"pub_side\":{pub_side},\"slot\":{slot_tag},\"expo\":{exposure},\"cause\":\"{cause}\"}}",
        kicked as u8
    );
    let mut guard = match LOADWIN_HISTORY.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.len() >= LOADWIN_HISTORY_CAP {
        guard.remove(0);
    }
    guard.push(record);
}
