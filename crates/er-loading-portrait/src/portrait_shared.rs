use crate::prelude::*;
use er_game_base::fnv1a::{fnv1a64, fnv1a64_extend};

// Shared portrait helpers used by the capture pipeline and its hosts: the per-window reset,
// the depth-mask invalidation, and the publish-acceptance classifiers. Relocated out of the
// path-A overlay composite (bd er-effects-rs-f9mq) because path B and the shared capture
// pipeline depend on them.

/// FNV-1a 64 over a character name's UTF-16 units (LE bytes), truncated to usize. The portrait
/// identity tag (bd er-effects-rs-dpf6 Phase 1): stamped at the game-thread build kick, copied next to
/// the bridge at publish, compared at the own-menu-switch rearm. 0 is reserved for "unknown/empty".
pub fn portrait_name_hash_utf16(units: &[u16]) -> usize {
    if units.is_empty() {
        return 0;
    }
    let mut h = fnv1a64(b"");
    for unit in units {
        h = fnv1a64_extend(h, &unit.to_le_bytes());
    }
    // Reserve 0 for "unknown" (an actual 0 hash is astronomically unlikely; map it to 1).
    (h as usize).max(1)
}

/// Name-hash of a ProfileSummary RECORD (name UTF-16 units at record+0). Game-thread only (guarded
/// game-memory read through the host seam). 0 = empty/unreadable name.
///
/// # Safety
///
/// No precondition on the address: every game read goes through the fault-tolerant
/// `safe_read_*` helpers, so 0, a freed pointer, or wholly unmapped memory returns the
/// empty/`None` result rather than faulting.
///
/// What the caller owns is INTERPRETATION: a successful read only proves those bytes
/// were mapped at that instant, not that they are the object this expects, and another
/// thread may overwrite them immediately afterwards. Treat the result as a sample.
///
/// Game thread only, because the record it samples is the one the game rewrites as a save
/// deserializes -- reading it from another thread yields a torn name, not a fault.
pub unsafe fn portrait_record_name_hash(record: usize) -> usize {
    let (units, len) = unsafe { read_utf16_name_units(record) };
    portrait_name_hash_utf16(&units[..len])
}

/// Name-hash of save `slot`'s ProfileSummary record (same record addressing as
/// `read_loading_screen_stats`). Game-thread only. 0 = unknown (no gdm/summary, bad slot, empty name).
///
/// # Safety
///
/// No precondition on the address: every game read goes through the fault-tolerant
/// `safe_read_*` helpers, so 0, a freed pointer, or wholly unmapped memory returns the
/// empty/`None` result rather than faulting.
///
/// What the caller owns is INTERPRETATION: a successful read only proves those bytes
/// were mapped at that instant, not that they are the object this expects, and another
/// thread may overwrite them immediately afterwards. Treat the result as a sample.
///
/// `slot` is range-checked before it is used to address a record. Game thread only, for
/// the same reason as `portrait_record_name_hash`.
pub unsafe fn portrait_slot_name_hash(slot: i32) -> usize {
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        return 0;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let gdm = game_data_man_ptr_or_null();
    if !valid(gdm) {
        return 0;
    }
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(0);
    if !valid(summary) {
        return 0;
    }
    let rec = profile_summary_record_address(summary, slot as usize);
    unsafe { portrait_record_name_hash(rec) }
}

/// Close the loading-portrait window: clear the published snapshot + the "have a head" gate so a later
/// window cannot flash the PREVIOUS character, drop the RT/depth candidate pins (the next window's
/// renderers are new objects), and clear the teardown-spared renderer so the NEXT load's teardown re-spares
/// the new character (LOADING_BG_PORTRAIT_SPARED_RENDERER is gated `== 0` and was otherwise never reset --
/// it stayed pinned to the first character's now-stale renderer, and driving that leaked renderer risks a
/// use-after-free). Idempotent. (Sole current caller: the own-menu-switch rearm, via
/// `loading_portrait_window_reset_for_switch`.)
pub fn loading_portrait_window_reset(reason: &str) {
    loading_portrait_window_reset_inner(reason, false)
}

/// Own-menu-switch variant (bd er-effects-rs-dpf6 Phase 3): if the INCOMING target identity
/// (slot + ProfileSummary name-hash) matches the currently-published head's identity tag, KEEP the
/// bridge and the frozen crop envelope across the rearm -- a same-character reload's cover shows the
/// held head from frame one instead of clearing it 0.1ms after RETARGET's make-before-break claimed it
/// holds. An identity MISMATCH (or unknown identity on either side) keeps the full 2026-07-06
/// wrong-character clear. Game-thread only (reads the incoming slot's summary record).
///
/// # Safety
///
/// The only game access is a guarded read of the incoming slot's ProfileSummary record to
/// hash its name, so `selected_slot` needs no precondition -- an out-of-range or
/// not-yet-populated slot simply hashes to 0 and takes the mismatch path.
///
/// Game thread only: this decides whether the published portrait survives the switch, and
/// it is ordered against the publish path by being on the same thread, not by a lock.
pub unsafe fn loading_portrait_window_reset_for_switch(selected_slot: i32, reason: &str) {
    // Reject attribution baseline (er-effects-rs-k979). LOADING_BG_PORTRAIT_RGBA_VERSION is
    // cumulative for the whole PROCESS -- it ends a 3-window run in the 1500s and never resets --
    // so "has anything published yet" is only meaningful against the CURRENT window. Snapshot it
    // here, at the one place a new portrait window begins, or every warm-up reject from switch 2
    // onward would be misfiled as a post-publish fault.
    er_telemetry::counters::LS_PORTRAIT_REJECT_PUBLISH_BASELINE.store(
        LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    // ONE WINDOW'S GRACE, NOT TWO. A hold still outstanding here rode the whole window that just
    // ended without ever publishing a frame of its own and without being revoked -- nothing
    // confirmed it and nothing refuted it. That is the 2026-08-22 `displayed-stale` shape (65
    // frames displayed, 0 published, 0 captured), and because the hold's own predicate compares one
    // record against itself, holding AGAIN on the same unchanged record would re-take it every
    // switch and let one stale head own window after window. Refuse the re-take instead: an
    // unproven head gets exactly one window, then the full wrong-character clear applies.
    //
    // Deliberately NOT a time or frame threshold. The legitimate bridge routinely covers seconds
    // (the measured confirm->publish latency is ~2.3s and windows 1/2/4 of that run published
    // 259-281 frames each after holding), and no measurement exists that separates "still waiting"
    // from "never coming" inside a window. "Did the last window ever prove it" needs no constant.
    let outstanding_hold = PORTRAIT_BRIDGE_HOLD_PROVISIONAL.swap(0, Ordering::SeqCst);
    if outstanding_hold != 0 {
        let n = PORTRAIT_BRIDGE_HOLD_UNPROVEN.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "loading-portrait: bridge hold UNPROVEN #{n} -- the previous window rode the held head (slot_tag={outstanding_hold}) start to finish without publishing its own frame and without being revoked; refusing to hold it a second window"
        ));
    }
    let incoming_hash = unsafe { portrait_slot_name_hash(selected_slot) };
    let incoming_slot_tag = if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&selected_slot) {
        (selected_slot + 1) as usize
    } else {
        0
    };
    let published_slot = LS_PORTRAIT_PUBLISHED_SLOT.load(Ordering::SeqCst);
    let published_hash = LS_PORTRAIT_PUBLISHED_NAME_HASH.load(Ordering::SeqCst);
    let have_head = PROFILE_HAVE_KEYED_FRAME.load(Ordering::SeqCst) != 0;
    // The predicate moved to `portrait_identity` unchanged, where a host test pins WHY a match here
    // is worth so little: `incoming_hash` and `published_hash` are the same ProfileSummary record
    // read at two different times, so a same-slot reselect matches by construction and the hold
    // cannot see that the record disagrees with the character that will actually load.
    let hold = outstanding_hold == 0
        && crate::portrait_identity::same_identity_bridge_hold(
            have_head,
            incoming_slot_tag,
            incoming_hash,
            published_slot,
            published_hash,
        );
    if hold {
        let n = PORTRAIT_BRIDGE_SAME_IDENTITY_HOLDS.fetch_add(1, Ordering::SeqCst) + 1;
        // PROVISIONAL from this instant. The independent signal that can falsify it -- the record's
        // face fingerprint vs the one the preview took from the picked save's own bytes -- does not
        // exist yet; it arrives at the first build kick, ~1.4s later on the measured machine. So the
        // hold is armed for revocation rather than trusted (`loading_portrait_bridge_hold_face_check`).
        PORTRAIT_BRIDGE_HOLD_PROVISIONAL.store(incoming_slot_tag, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "loading-portrait: same-identity bridge HOLD #{n} across switch rearm (slot_tag={incoming_slot_tag} name_hash=0x{incoming_hash:x}) -- PROVISIONAL: keeping published head + crop envelope until this window publishes its own frame or the face fingerprint revokes it"
        ));
    }
    loading_portrait_window_reset_inner(reason, hold);
}

/// Drop the published head and the frozen crop envelope: after this the loading screen has NO
/// portrait to draw until something publishes a new one.
///
/// Extracted so the window reset's wrong-character clear and a mid-window hold revocation do
/// literally the same thing. They must: a revocation is the clear that the reset should have
/// performed and could not, because the evidence for it had not arrived yet.
fn loading_portrait_drop_published_bridge() {
    if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
        *g = None;
    }
    PROFILE_HAVE_KEYED_FRAME.store(0, Ordering::SeqCst);
    // Identity tag lives-and-dies with the bridge content.
    LS_PORTRAIT_PUBLISHED_SLOT.store(0, Ordering::SeqCst);
    LS_PORTRAIT_PUBLISHED_NAME_HASH.store(0, Ordering::SeqCst);
    // Crop envelope: re-seed for the NEW character's silhouette (it was frozen after the first
    // PORTRAIT_CROP_SEED_N frames and previously never reset, so a different character inherited
    // the prior head's rect).
    PORTRAIT_CROP_MINX.store(usize::MAX, Ordering::SeqCst);
    PORTRAIT_CROP_MINY.store(usize::MAX, Ordering::SeqCst);
    PORTRAIT_CROP_MAXX.store(0, Ordering::SeqCst);
    PORTRAIT_CROP_MAXY.store(0, Ordering::SeqCst);
    PORTRAIT_CROP_SEED_FRAMES.store(0, Ordering::SeqCst);
}

/// Resolve an outstanding provisional bridge hold against the build kick's record-vs-preview FACE
/// fingerprint. Returns true when the hold was revoked. Game thread (the caller has just read the
/// record); no-op when no hold is outstanding.
///
/// WHY THE HOLD NEEDED AN OUTSIDE SIGNAL AT ALL. Every other identity check the portrait pipeline
/// runs reads the ProfileSummary record on BOTH sides of its comparison -- the hold's name hashes,
/// the published-vs-target name hashes, the loadwin `identity=` tag -- so all of them agree with
/// themselves no matter how wrong the record is. Run br-20260822-040913-f0f4 is the demonstration:
/// window #3 closed `identity=ok` while this fingerprint had already disagreed twice, and the
/// identity semaphore separately logged slot 0's record naming `Maddened Bean` while the resident
/// character was `Ordinary Bean`. The face fingerprint is the only value in the pipeline taken from
/// outside the record (the picked save's own bytes, at preview time), so it is the only one that
/// can say the record is wrong -- and the drift it reported that day is itself the proof the record
/// had been rewritten since the preview stamped it.
///
/// WHY REVOCATION RATHER THAN A BETTER HOLD PREDICATE. This signal does not exist when the hold is
/// taken. The rearm ran at +107006ms; the first fingerprint comparison ran at +108385ms. Folding it
/// into the rearm decision is not available -- the only options at rearm are the record-derived
/// hashes that cannot fail. So the hold is taken optimistically (make-before-break is worth
/// keeping) and armed to be taken back.
///
/// Racing the publish worker is benign in both directions: if the worker publishes this window's
/// own frame first, the latch is already clear and this is a no-op; if it publishes immediately
/// after a revocation, the bridge simply holds that fresh frame instead. The one outcome that
/// cannot happen is the previous character's head surviving a refutation.
pub fn loading_portrait_bridge_hold_face_check(
    kick_slot: i32,
    record_face_hash: usize,
    preview_face_hash: usize,
) -> bool {
    let held = PORTRAIT_BRIDGE_HOLD_PROVISIONAL.load(Ordering::SeqCst);
    let verdict = crate::portrait_identity::bridge_hold_face_verdict(
        held,
        kick_slot,
        record_face_hash,
        preview_face_hash,
    );
    if !verdict.revokes() {
        return false;
    }
    // Clear the latch first: a second kick for the same slot (the run logged two, 3.6s apart) must
    // not count as a second revocation of a head that is already gone.
    if PORTRAIT_BRIDGE_HOLD_PROVISIONAL.swap(0, Ordering::SeqCst) == 0 {
        return false;
    }
    loading_portrait_drop_published_bridge();
    // The held head's silhouette is gone with it, so the cached depth mask must not key the next
    // frame with the previous character's cutout.
    invalidate_portrait_depth_mask();
    let n = PORTRAIT_BRIDGE_HOLD_REVOCATIONS.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "loading-portrait: bridge hold REVOKED #{n} at build kick slot={kick_slot} -- record face hash 0x{record_face_hash:x} != preview 0x{preview_face_hash:x}, so the held head is not the incoming character; dropped the published head + crop envelope (this loading screen shows NO portrait until one publishes, which is correct -- a wrong face is worse than none)"
    ));
    true
}

/// A publish of THIS window's own frame supersedes any provisional hold: the bridge no longer
/// holds a previous window's head, so there is nothing left to revoke. Called from the depth-keyed
/// worker publish, which is the write that `PORTRAIT-LOADWIN VERDICT`'s `publishes=` counts; the
/// two colour-only bridge writers deliberately do NOT clear it, because neither bumps
/// `LOADING_BG_PORTRAIT_RGBA_VERSION` and both build from the same possibly-wrong record.
pub fn loading_portrait_bridge_hold_superseded_by_publish() {
    PORTRAIT_BRIDGE_HOLD_PROVISIONAL.store(0, Ordering::SeqCst);
}

fn loading_portrait_window_reset_inner(reason: &str, hold_bridge: bool) {
    // WORKER-OFFLOAD SWITCH SAFETY (2026-07-06). Bump the pipeline generation FIRST: any portrait consume
    // job still in flight on the worker thread snapshotted the PREVIOUS gen, so when it re-reads this before
    // it pins/publishes it will see the bump and DISCARD -- a head captured for the old window can never be
    // pinned/published into the new one.
    PORTRAIT_PIPELINE_GEN.fetch_add(1, Ordering::SeqCst);
    // Then bounded-drain the in-flight consume jobs (up to ~15ms, yielding) so late telemetry lands in the
    // right window and no worker is mid-publish while we clear the state below. This reset already runs off
    // the render thread (see the note further down), so a short spin here is safe.
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(15);
        while PORTRAIT_JOB_INFLIGHT.load(Ordering::SeqCst) != 0
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
    }
    // CLEAR-ON-COMPLETE (user 2026-07-06, REVERSING the 2026-07-03 make-before-break KEEP): drop the
    // published head snapshot the moment the load completes (character in-world, native bar terminal).
    // The kept bridge was the stale-content reservoir behind the second-load wrong-head bug: the next
    // window's forge baked the PREVIOUS character's held frame into the now-loading background at decode
    // time (the bake was decode-once), so the old head
    // stayed on screen for the whole next load even while the readback/publish pipeline was proven
    // (pixel-diff vs same-character baseline, runs 2026-07-06) to produce the NEW character. With the
    // snapshot cleared here there is nothing stale to bake or bridge: the next window starts head-less
    // and shows the new character's first keyed frame. Costs a brief head-less loading screen
    // (~0.5s after the window's table build in both measured runs) -- preferred over a wrong head.
    //
    // SAME-IDENTITY HOLD (bd er-effects-rs-dpf6 Phase 3): when the caller PROVED the incoming target
    // is the SAME character as the published head (slot + name-hash tag match), keeping the bridge
    // cannot show a wrong head -- it shows the right head a full publish-latency (~4s from confirm on
    // the measured machine) earlier. Only the bridge, its identity tag, and the frozen crop envelope
    // are kept; every per-window counter/pin below still resets.
    //
    // On a same-identity hold the bridge and its frozen crop envelope stay put -- same character,
    // same silhouette. `loading_portrait_drop_published_bridge` is the same clear a mid-window hold
    // REVOCATION performs, which is the point of sharing it: a revocation is exactly this clear,
    // run late, once the evidence the rearm did not have finally arrives.
    if !hold_bridge {
        loading_portrait_drop_published_bridge();
    }
    PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
    PROFILE_LOADSCREEN_TABLE_OWNED.store(0, Ordering::SeqCst);
    // Rebuild the stats text for the next load (a System-Quit character switch may load a different char).
    stats_text_window_reset();
    PROFILE_RT_PIN.store(0, Ordering::SeqCst);
    PROFILE_DEPTH_PIN.store(0, Ordering::SeqCst);
    // Fresh adaptive tear baseline for the next window's character (honest content scores differ
    // per character: speckled textures sit ~40, smooth skin ~3).
    PROFILE_TEAR_EMA.store(0, Ordering::SeqCst);
    // Do NOT drop the spared renderer -- that leaked one live CSMenuProfModelRend per switch (it was
    // excluded from the native delete and its offscreen draw task kept filling the 192-slot GX
    // command queue -> 0x1aeaf05 overflow ~switch #4). MOVE it to the orphan slot; the game-thread
    // teardown-spare hook delete-enqueues it via CSDelayDeleteMan at the next teardown (this reset
    // runs off the game thread, so it stashes rather than deleting in place).
    let prev_spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.swap(0, Ordering::SeqCst);
    if prev_spared != 0 {
        PROFILE_SPARE_ORPHAN.store(prev_spared, Ordering::SeqCst);
    }
    PROFILE_SPARE_CANDIDATE.store(0, Ordering::SeqCst);
    // Re-arm the idle-anim bind + drop the motion-metric history so the NEXT load window binds its
    // own renderer and starts a fresh inter-frame diff (cumulative attempt/max oracles are kept).
    PORTRAIT_ANIM_BIND_STATE.store(0, Ordering::SeqCst);
    PORTRAIT_ANIM_BOUND_RENDERER.store(0, Ordering::SeqCst);
    PORTRAIT_ANIM_BOUND_LOC.store(0, Ordering::SeqCst);
    PORTRAIT_KICK_SLOT_KEY.store(0, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(0, Ordering::SeqCst);
    // Release this window's committed portrait target so the NEXT load is free to name a different
    // character. Without this reset the latch would pin the boot character's face across every
    // later System->Quit->Load switch -- the same wrong-face class in the opposite direction.
    PORTRAIT_WINDOW_TARGET_SLOT.store(0, Ordering::SeqCst);
    // The kick-target name hash re-stamps at the next window's build kick (both modes).
    PORTRAIT_TARGET_NAME_HASH.store(0, Ordering::SeqCst);
    if let Ok(mut g) = PORTRAIT_MOTION_PREV_PLANES.lock() {
        *g = None;
    }
    if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
        *g = None;
    }
    // Cache cleared -> forget which character it was for (a fresh compute re-tags it).
    LAST_DEPTH_MASK_INCARNATION.store(0, Ordering::SeqCst);
    // Animation-stall semaphore: snapshot this window's animated-vs-displayed frame counts, then zero
    // for the next window. drive << display == the head froze early (freeze-after-capture); the
    // user's "stopped animating / frozen the whole loading screen" symptom shows here as a low ratio.
    let drive = PROFILE_DRIVE_FRAMES_WINDOW.swap(0, Ordering::SeqCst);
    let display = PROFILE_DISPLAY_FRAMES_WINDOW.swap(0, Ordering::SeqCst);
    // `PROFILE_RT_SRV_COPIES_WINDOW` is a true per-window counter used by the fast-fail gate. Reset it
    // here with the other per-window counters; leaving stale copies from an earlier window makes a later
    // no-copy failure report as cause=0/unknown instead of the actionable no-copy class.
    let copies = PROFILE_RT_SRV_COPIES_WINDOW.swap(0, Ordering::SeqCst);
    PROFILE_DRIVE_FRAMES_WINDOW_LAST.store(drive, Ordering::SeqCst);
    PROFILE_DISPLAY_FRAMES_WINDOW_LAST.store(display, Ordering::SeqCst);
    // PUBLISH-STARVATION ATTRIBUTION (2026-07-03 soak: windows froze on the PRIOR character with the
    // drive running ~1:1, so the starving class is publish-side and the cumulative oracles cannot say
    // WHICH window starved or WHY). Snapshot each publish/skip class per window (delta vs the previous
    // reset) so a frozen window names its own cause: published==0 with a dominant torn/unkeyed/multi
    // count is the starvation signature; pin_moves counts content-RT recreations inside the window.
    let winof = |cum: &AtomicUsize, last: &AtomicUsize| -> usize {
        let c = cum.load(Ordering::SeqCst);
        c.saturating_sub(last.swap(c, Ordering::SeqCst))
    };
    let published = winof(&PROFILE_PUBLISH_CLEAN, &PROFILE_PUBLISH_CLEAN_WINDOW_MARK);
    let torn = winof(
        &PROFILE_PUBLISH_SKIPPED_TORN,
        &PROFILE_PUBLISH_SKIPPED_TORN_WINDOW_MARK,
    );
    let unkeyed = winof(
        &PROFILE_PUBLISH_SKIPPED_UNKEYED,
        &PROFILE_PUBLISH_SKIPPED_UNKEYED_WINDOW_MARK,
    );
    let multi = winof(
        &PROFILE_MULTI_MODEL_PUBLISH_SKIPS,
        &PROFILE_MULTI_MODEL_PUBLISH_SKIPS_WINDOW_MARK,
    );
    let pin_moves = winof(
        &PROFILE_RT_PIN_SWITCHES,
        &PROFILE_RT_PIN_SWITCHES_WINDOW_MARK,
    );
    let fence_skips = winof(
        &PROFILE_DRIVE_FENCE_SKIPS,
        &PROFILE_DRIVE_FENCE_SKIPS_WINDOW_MARK,
    );
    // Source provenance per window: cb/cs = color ticks resolved from the scene bundle vs the scan;
    // dc/db = depth via the deterministic chain vs the BFS; unpaired = real frames held back for
    // lacking bundle provenance (the green-face wrong-buffer class). A starved window (clean=0)
    // with cs/db dominant convicts a chain miss for that window's renderer.
    let cb = winof(
        &PROFILE_COLOR_FROM_BUNDLE,
        &PROFILE_COLOR_FROM_BUNDLE_WINDOW_MARK,
    );
    let cs = winof(
        &PROFILE_COLOR_FROM_SCAN,
        &PROFILE_COLOR_FROM_SCAN_WINDOW_MARK,
    );
    let dc = winof(
        &PROFILE_DEPTH_FROM_CHAIN,
        &PROFILE_DEPTH_FROM_CHAIN_WINDOW_MARK,
    );
    let db = winof(&PROFILE_DEPTH_FROM_BFS, &PROFILE_DEPTH_FROM_BFS_WINDOW_MARK);
    let unpaired = winof(
        &PROFILE_PUBLISH_SKIPPED_UNPAIRED,
        &PROFILE_PUBLISH_SKIPPED_UNPAIRED_WINDOW_MARK,
    );
    let lowmask = winof(
        &PROFILE_PUBLISH_SKIPPED_LOWMASK,
        &PROFILE_PUBLISH_SKIPPED_LOWMASK_WINDOW_MARK,
    );
    // First-keyed latency: display-frame index of this window's first publish ('-' = never
    // published; the whole window rode the bridge). Snapshot + re-arm for the next window.
    let first_keyed = PROFILE_WINDOW_FIRST_KEYED_DISPLAY.swap(usize::MAX, Ordering::SeqCst);
    PROFILE_WINDOW_FIRST_KEYED_DISPLAY_LAST.store(
        if first_keyed == usize::MAX {
            0
        } else {
            first_keyed
        },
        Ordering::SeqCst,
    );
    let first_keyed_s = if first_keyed == usize::MAX {
        "-".to_owned()
    } else {
        first_keyed.to_string()
    };
    // Floor-evidence: min transparent share among floor-passing frames vs max among lowmask-held
    // frames this window ('-' = no frame in that class). Sets PORTRAIT_MIN_TRANSPARENT_PCT from data.
    let share_min = PROFILE_PUBLISH_SHARE_MIN.swap(usize::MAX, Ordering::SeqCst);
    let share_min_s = if share_min == usize::MAX {
        "-".to_owned()
    } else {
        share_min.to_string()
    };
    let held_max = PROFILE_LOWMASK_SHARE_MAX.swap(0, Ordering::SeqCst);
    let checker = winof(
        &PROFILE_READBACK_CHECKER,
        &PROFILE_READBACK_CHECKER_WINDOW_MARK,
    );
    let badiou = winof(
        &PROFILE_PUBLISH_SKIPPED_BADIOU,
        &PROFILE_PUBLISH_SKIPPED_BADIOU_WINDOW_MARK,
    );
    // HARNESS-FAILURE semaphore (user directive 2026-07-06): a window that DROVE the model (produced
    // readback frames) yet published ZERO clean portraits is a broken feature for that character, not an
    // acceptable silent skip. The FAST-FAIL in the draw tick trips this mid-window (grace=0) so it fires
    // the frame the render misses, not here at window close. This is the BACKSTOP: it records the
    // precise per-window dominant cause for the log, and only increments the failure counter if the
    // fast-fail latch did not already count this window (defensive; ~never with grace=0). Guarded on
    // `drive > 0` -- a window that never got a model (build-side gap) is not a publish-gate fault.
    if published == 0 && drive > 0 {
        let cause = if torn >= unkeyed && torn >= badiou && torn >= lowmask && torn > 0 {
            1 // torn: usable frames the tear metric rejected
        } else if unkeyed >= badiou && unkeyed >= lowmask && unkeyed > 0 {
            2 // unkeyed: depth mask never cut background (opaque/black RT)
        } else if badiou >= lowmask && badiou > 0 {
            3
        } else if lowmask > 0 {
            4
        } else if checker > 0 {
            5
        } else if multi > 0 {
            6
        } else if unpaired > 0 {
            7
        } else if copies == 0 {
            8
        } else if cb + cs + dc + db == 0 {
            9
        } else {
            0
        };
        PORTRAIT_WINDOW_PUBLISH_FAIL_CAUSE.store(cause, Ordering::SeqCst);
        let already = PORTRAIT_WINDOW_PUBLISH_FAIL_LATCHED.load(Ordering::SeqCst) != 0;
        let n = if already {
            PORTRAIT_WINDOW_PUBLISH_FAILURES.load(Ordering::SeqCst)
        } else {
            PORTRAIT_WINDOW_PUBLISH_FAILURES.fetch_add(1, Ordering::SeqCst) + 1
        };
        append_autoload_debug(format_args!(
            "present-overlay: PORTRAIT PUBLISH FAILURE #{n}{} -- window drove {drive} frames but published 0 (dominant cause={} torn={torn} unkeyed={unkeyed} badiou={badiou} lowmask={lowmask} checker={checker} multi={multi} unpaired={unpaired} copies={copies} cb={cb} cs={cs} dc={dc} db={db}); HARNESS MUST FAIL until the root render is fixed",
            if already {
                " (already fast-failed)"
            } else {
                ""
            },
            match cause {
                1 => "torn",
                2 => "unkeyed",
                3 => "badiou",
                4 => "lowmask",
                5 => "checker",
                6 => "multi",
                7 => "unpaired",
                8 => "no-copy",
                9 => "no-provenance",
                _ => "unknown",
            }
        ));
    }
    // Re-arm the per-window fast-fail state for the next window.
    PROFILE_PUBLISH_CLEAN_WINDOW.store(0, Ordering::SeqCst);
    PORTRAIT_WINDOW_PUBLISH_FAIL_LATCHED.store(0, Ordering::SeqCst);
    PORTRAIT_LAST_SKIP_CLASS.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "present-overlay: loading-portrait window reset ({reason}{}) -- animated {drive} / displayed {display} frames (drive<<display == froze early); publish[clean={published} torn={torn} unkeyed={unkeyed} lowmask={lowmask} badiou={badiou} checker={checker} multi={multi} pin_moves={pin_moves} fence_skips={fence_skips} unpaired={unpaired} copies={copies} first_keyed={first_keyed_s}] share[pass_min={share_min_s} held_max={held_max}] src[color bundle={cb}/scan={cs} depth chain={dc}/bfs={db}] (clean=0 with drive>0 == PUBLISH FAILURE, see the failure line above; the dominant skip class is the cause); pins/spare cleared for the next load",
        if hold_bridge {
            ", same-identity bridge HELD"
        } else {
            ""
        }
    ));
}

/// Invalidate the depth-key MASKING PLANE for a NEW model: drop the cached mask and the pinned depth
/// candidate so the next `apply_depth_alpha_key` RECOMPUTES the silhouette from the new model's own depth
/// buffer instead of reusing the previous character's cached mask. Without this, a System Quit -> Load
/// Profile character switch would cut the OLD character's silhouette out of the NEW head until fresh depth
/// happened to land. Fail-open in the gap (leaves the head opaque) -- never a stale wrong-shape cutout.
pub fn invalidate_portrait_depth_mask() {
    PROFILE_DEPTH_PIN.store(0, Ordering::SeqCst);
    if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
        *g = None;
    }
    // Cache cleared -> forget which character it was for (a fresh compute re-tags it).
    LAST_DEPTH_MASK_INCARNATION.store(0, Ordering::SeqCst);
}

/// True if the read-back RGBA8 image has any non-black texel (`max(R,G,B) > 24`) inside a center
/// 64x64 region. Used to set `LOADING_BG_PORTRAIT_NONBLACK` -- a quick "did we capture a real head
/// vs a blank/black offscreen" oracle.
pub fn portrait_center_nonblack(width: u32, height: u32, pixels: &[u8]) -> bool {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h * RGBA8_BPP {
        return false;
    }
    const REGION: usize = 64;
    let half = REGION / 2;
    let cx = w / 2;
    let cy = h / 2;
    let x0 = cx.saturating_sub(half);
    let x1 = (cx + half).min(w);
    let y0 = cy.saturating_sub(half);
    let y1 = (cy + half).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = (y * w + x) * RGBA8_BPP;
            let r = pixels[idx];
            let g = pixels[idx + 1];
            let b = pixels[idx + 2];
            if r.max(g).max(b) > 24 {
                return true;
            }
        }
    }
    false
}

/// True if the read-back RGBA8 image looks like a SOLID-COLOR-CHECKER PLACEHOLDER (our magenta/white or
/// magenta/yellow er-tpf cover, or an unrendered RT clear pattern) rather than a real 3D head render.
///
/// WHY: `portrait_center_nonblack` only proves "not all black" -- a bright magenta checker (255,0,255)
/// trivially passes it, so `oracle_loading_bg_portrait_gx_nonblack` was a FALSE POSITIVE for the autoload
/// path (run postcontinue-lookat-smoke 2026-06-30: nonblack=True but the captured bytes were a magenta/
/// white checker, because the model builds but is never rendered into the offscreen RT once the menu's
/// render driver dies post-Continue). A real character render has many shaded colors and few fully-
/// saturated "pure" texels; a checker is ~2 colors, each with channels pinned to 0/255. Heuristic over the
/// center region: sample texels, quantize to 5 bits/channel, and call it a checker if (a) the 2 most-common
/// quantized colors cover >= 85% of samples AND (b) >= 70% of samples are "pure" (every channel <16 or >239).
pub fn portrait_looks_like_checker(width: u32, height: u32, pixels: &[u8]) -> bool {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h * RGBA8_BPP {
        return false;
    }
    const REGION: usize = 128;
    let half = REGION / 2;
    let (cx, cy) = (w / 2, h / 2);
    let x0 = cx.saturating_sub(half);
    let x1 = (cx + half).min(w);
    let y0 = cy.saturating_sub(half);
    let y1 = (cy + half).min(h);
    let mut counts: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
    let mut total = 0u32;
    let mut pure = 0u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = (y * w + x) * RGBA8_BPP;
            let (r, g, b) = (pixels[idx], pixels[idx + 1], pixels[idx + 2]);
            // pure = every channel near an extreme (0/255) -> checker/placeholder hallmark
            let is_pure = |c: u8| !(16..=239).contains(&c);
            if is_pure(r) && is_pure(g) && is_pure(b) {
                pure += 1;
            }
            let key = (((r >> 3) as u16) << 10) | (((g >> 3) as u16) << 5) | ((b >> 3) as u16);
            *counts.entry(key).or_insert(0) += 1;
            total += 1;
        }
    }
    if total == 0 {
        return false;
    }
    let mut vals: Vec<u32> = counts.values().copied().collect();
    vals.sort_unstable_by(|a, b| b.cmp(a));
    let top2: u32 = vals.iter().take(2).sum();
    let top2_frac = top2 as f32 / total as f32;
    let pure_frac = pure as f32 / total as f32;
    top2_frac >= 0.85 && pure_frac >= 0.70
}
