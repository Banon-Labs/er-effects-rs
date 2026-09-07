// The birth of the wedge, timestamped -- and the host clock that lets the stamp be read against
// the rest of a run's evidence.
//
// `include!`d into `lib.rs` like the blocks around it, and split out of it because the
// first-occurrence instrument is a different question from the observers that feed it.
// `save_orphan_drain.rs` names the dead state; `save_state_witness.rs` names the frame that creates
// it at the two `saveState` WRAPPERS; this file names the first frame the wedge is visible from the
// dispatch, which is the only vantage point that keeps working once the wrappers stop being called.
//
// Why first and not last. `DECLINE_SLOT` / `DECLINE_SAVE_STATE` in `lib.rs` are last-writer-wins.
// On the 2026-08-31 run that meant the published sample was taken on decline 8,638 of 8,638 -- four
// minutes after the state it describes came into being, and byte-identical to the 8,637 before it,
// because a declining lane touches nothing and so nothing in that loop can change the device. A
// sample of a fixpoint says what the fixpoint is; it cannot say when or with what it started.
//
// No new hook. Every value here comes from the sample `observe_dispatch` already takes on a
// decline; the only change is which occurrence is kept.

/// Signature of the clock sink: milliseconds since the host's own log epoch.
///
/// A third sink rather than `Instant::now()` inside this crate, for one reason: a timestamp is
/// only useful if it can be lined up against the other evidence a run produces. The host's epoch
/// is the one every `[+<n>ms]` prefix in `er-quickload-autoload-debug.log` and every record in
/// `er-quickload-continue-trace.log` already counts from, so a stamp taken through this sink can
/// be read straight against the accept record that latched the device. A private clock would be
/// correct and useless.
pub type ClockSinkFn = fn() -> u64;
static CLOCK_SINK: AtomicUsize = AtomicUsize::new(0);

/// Install the clock sink. Call once, before [`install`]/[`install_observers_only`]. Optional:
/// without it every stamp reads [`ELAPSED_MS_UNAVAILABLE`], which is reported as "unstamped"
/// rather than as a time.
pub fn set_clock_sink(sink: ClockSinkFn) {
    CLOCK_SINK.store(sink as usize, Ordering::Release);
}

/// Sentinel for a stamp taken with no clock sink wired -- Not a moment in time.
pub const ELAPSED_MS_UNAVAILABLE: u64 = u64::MAX;

/// Milliseconds since the host's log epoch, or [`ELAPSED_MS_UNAVAILABLE`] when no sink is wired.
fn elapsed_ms() -> u64 {
    let raw = CLOCK_SINK.load(Ordering::Acquire);
    if raw == 0 {
        return ELAPSED_MS_UNAVAILABLE;
    }
    // SAFETY: `raw` is only ever a `ClockSinkFn` stored by `set_clock_sink`.
    let sink: ClockSinkFn = unsafe { std::mem::transmute::<usize, ClockSinkFn>(raw) };
    sink()
}

// ---- The birth of the wedge, timestamped -------------------------------------------------
//
// `DECLINE_SLOT` / `DECLINE_SAVE_STATE` above are last-writer-wins. On the 2026-08-31 run that
// meant the published sample was taken on decline 8,638 of 8,638 -- four minutes after the state
// it describes came into being, and byte-identical to the 8,637 before it because nothing in that
// loop can change the device. A sample of a fixpoint says what the fixpoint is; it cannot say when
// or with what it started.
//
// These are first-writer-wins over exactly the condition that defines the wedge, so they name the
// moment instead of the plateau. Comparing the captured `+0x10` against the accept records in
// `er-quickload-continue-trace.log` (which log the `SLSaveContent` the accepting lane built) says
// outright which submit is the one still on the device -- the reload's own SetState5 autosave, or
// something later -- and the stamp shares the host's log epoch, so it lands between two named
// lines of the debug log rather than in the abstract.
//
// No new hook. This is the same `observe_dispatch` sample the decline path already takes, recorded
// on first occurrence instead of last.

/// Declines whose sample showed a save latched on a device the game thinks is idle.
///
/// The plateau, counted. Read beside `DISPATCH_DECLINES`: nearly-equal means the wedge existed for
/// essentially the whole run, a small number against a large decline count means the device was
/// briefly latched and recovered.
static DISPATCH_LATCHED_DECLINES: AtomicU64 = AtomicU64::new(0);
/// 0 until the first wedged sample is recorded. Distinguishes "never seen" from "seen, all zero",
/// which no combination of the pointer fields can do on its own.
static DISPATCH_FIRST_LATCHED_SEEN: AtomicUsize = AtomicUsize::new(0);
/// The device as it stood at that first wedged sample -- `+0x10` and `+0x20` are the two the
/// builders' precondition tests, and the two that identify the submit.
static DISPATCH_FIRST_LATCHED_SLOT: SlotSampleCell = SlotSampleCell::new();
/// Host-epoch milliseconds at that first wedged sample ([`ELAPSED_MS_UNAVAILABLE`] = unstamped).
static DISPATCH_FIRST_LATCHED_MS: AtomicU64 = AtomicU64::new(ELAPSED_MS_UNAVAILABLE);
/// Which dispatch entry it was (1-based, counting all lanes). Says how early in the run the wedge
/// formed without needing the log.
static DISPATCH_FIRST_LATCHED_CALL: AtomicU64 = AtomicU64::new(0);
/// `SAVE_LANE_*` of that first wedged sample.
static DISPATCH_FIRST_LATCHED_LANE: AtomicUsize = AtomicUsize::new(SAVE_LANE_NONE);

/// Does this dispatch sample show a save the game has stopped believing in?
///
/// `iodev+0x10 != 0` with `saveState == 0`: the device holds save content, and the mutex that
/// would make anything poll it is free. Every submit path sets `saveState = 1` at its commit tail
/// and only `FUN_140e6f200` clears `+0x10`, so the pair cannot occur in a healthy transaction --
/// it is the wedge, seen from the dispatch that the wedge is refusing.
///
/// Deliberately LOOSER than [`save_request_is_orphaned`], and the difference is not an oversight.
/// That predicate gates a call into the engine, so it additionally demands `load_content == 0` and
/// `file_cap == 0` -- release the device while the load side or a deferred build also owns it and
/// `FUN_140e6f200` frees their objects too. This one gates a counter and a log line, so narrowing
/// it the same way would make the instrument blind to precisely the compound states hardest to
/// reason about, for a safety margin nothing here needs.
///
/// Pure and total, so the rule that decides what counts as the birth of the wedge is exercised on
/// the host with no game attached. An unreadable device or `GameMan` is never a finding: "unknown"
/// is not evidence, in either direction.
pub fn dispatch_sample_is_wedged(slot: Option<SlRequestSlot>, save_state: Option<u32>) -> bool {
    let (Some(slot), Some(save_state)) = (slot, save_state) else {
        return false;
    };
    slot.save_content != 0 && save_state == GAME_MAN_SAVE_STATE_IDLE
}

/// Dispatch declines whose sample matched [`dispatch_sample_is_wedged`].
///
/// Read it against [`dispatch_declines`]. Zero, with declines recorded, says every refusal that run
/// happened with the device not holding an abandoned save -- which rules the wedge out as their
/// cause rather than leaving it open.
pub fn dispatch_latched_declines() -> u64 {
    DISPATCH_LATCHED_DECLINES.load(Ordering::SeqCst)
}

/// The device at the first wedged dispatch sample, or `None` when none was ever seen.
///
/// `save_content` (`iodev+0x10`) is the `SLSaveContent` of the submit that is still on the device.
/// Compare it against the accept records to name which submit wedged; `job` (`iodev+0x20`) says
/// whether that submit reached `FUN_140e6fb50` (non-zero) or was deferred/never enqueued (zero).
pub fn dispatch_first_latched_slot() -> Option<SlRequestSlot> {
    (DISPATCH_FIRST_LATCHED_SEEN.load(Ordering::SeqCst) != 0)
        .then(|| DISPATCH_FIRST_LATCHED_SLOT.snapshot())
        .flatten()
}

/// Host-epoch milliseconds at that first wedged sample, or [`ELAPSED_MS_UNAVAILABLE`] when it was
/// never seen or no clock sink was wired. Read [`dispatch_first_latched_slot`] to tell those apart.
pub fn dispatch_first_latched_ms() -> u64 {
    DISPATCH_FIRST_LATCHED_MS.load(Ordering::SeqCst)
}

/// Dispatch entry number (1-based, all lanes) of that first wedged sample; 0 = never seen.
pub fn dispatch_first_latched_call() -> u64 {
    DISPATCH_FIRST_LATCHED_CALL.load(Ordering::SeqCst)
}

/// `SAVE_LANE_*` of that first wedged sample.
pub fn dispatch_first_latched_lane() -> usize {
    DISPATCH_FIRST_LATCHED_LANE.load(Ordering::SeqCst)
}

/// Record one wedged dispatch sample, latching the first one.
///
/// Cold code the hot path only jumps to, like `note_abandoning_write`. The log line is emitted on
/// the first occurrence alone: the state is a fixpoint, so every later frame would repeat it
/// verbatim for as long as the process lives.
#[cfg(windows)]
fn note_wedged_dispatch(slot: Option<SlRequestSlot>, lane: usize, call: u64) {
    let total = DISPATCH_LATCHED_DECLINES.fetch_add(1, Ordering::SeqCst) + 1;
    if DISPATCH_FIRST_LATCHED_SEEN.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let stamp = elapsed_ms();
    // Freeze the writer counters at the same first occurrence. Read against them, the birth stops
    // being "a wedge exists" and becomes "a wedge exists and N witnessed writes had taken saveState
    // off 1 by then" -- which at N = 0 is a verdict about the whole writer set.
    snapshot_writers_at_wedge();
    // ...and attribute the content the device is holding to the submit that put it there. Same
    // first-wins sample, no extra read: `save_submit_latch.rs` answers "which submit latched this,
    // and did its lane accept", which is the one question a writer counter cannot ask.
    if let Some(sample) = slot {
        snapshot_latch_at_wedge(sample.save_content);
    }
    store_slot_sample(slot, &DISPATCH_FIRST_LATCHED_SLOT);
    DISPATCH_FIRST_LATCHED_MS.store(stamp, Ordering::SeqCst);
    DISPATCH_FIRST_LATCHED_CALL.store(call, Ordering::SeqCst);
    DISPATCH_FIRST_LATCHED_LANE.store(lane, Ordering::SeqCst);
    log_message(format_args!(
        "suppress: WEDGE BORN (#{total}) at dispatch entry {call}, lane {lane} -- the SL device \
         holds a save (iodev+0x10 != 0) while GameMan.saveState is 0, so nothing will poll it and \
         every later submit fails `iodev+0x10 == 0 && iodev+0x20 == 0`. Stamp: {}. Device: {}",
        match stamp {
            ELAPSED_MS_UNAVAILABLE => "unstamped (no clock sink wired)".to_owned(),
            ms => format!("+{ms}ms"),
        },
        describe_slot(slot)
    ));
    publish_snapshot();
}

// The predicate is the whole reviewable surface of an instrument that otherwise only exists at
// runtime, so it is tested with no game attached -- same reason as `poll_abandoned_a_save`.
#[cfg(test)]
mod save_wedge_birth_tests {
    use super::*;

    /// The rule that decides what counts as the birth of the wedge, and every way of not crying
    /// wolf about it. It is the one thing in the first-occurrence instrument that can be wrong
    /// without a game attached, so it is checked without one.
    #[test]
    fn only_a_save_the_game_stopped_believing_in_is_a_wedge() {
        let clear = SlRequestSlot::default();
        let latched = SlRequestSlot {
            // The measured 2026-08-31 signature.
            save_content: 0x9ad1_d280,
            job: 0x1_8e31_9ea0,
            ..clear
        };

        // The EVENT: content on the device, mutex free, so nothing will ever poll it.
        assert!(dispatch_sample_is_wedged(
            Some(latched),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        // An ordinary in-flight save. `DoSaveStuff` polls this every frame under IsSaveState1 --
        // reporting it would drown the real event in the healthy case.
        assert!(!dispatch_sample_is_wedged(
            Some(latched),
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS)
        ));
        // Load phases own the transaction too; a save cannot even submit at 2.
        assert!(!dispatch_sample_is_wedged(
            Some(latched),
            Some(GAME_MAN_SAVE_STATE_LOAD_OWNS)
        ));
        assert!(!dispatch_sample_is_wedged(
            Some(latched),
            Some(GAME_MAN_SAVE_STATE_LOAD_RESIDENT)
        ));
        // An idle device with an idle mutex is the normal resting state of the whole subsystem.
        assert!(!dispatch_sample_is_wedged(
            Some(clear),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        // Unknown is never evidence, on either side.
        assert!(!dispatch_sample_is_wedged(None, Some(GAME_MAN_SAVE_STATE_IDLE)));
        assert!(!dispatch_sample_is_wedged(Some(latched), None));

        // Deliberately LOOSER than the drain'S PREDICATE. `save_request_is_orphaned` gates a call
        // into the engine and so refuses a device the load side or a deferred build also owns;
        // this one gates a counter, and must still see those compound states.
        let compound = SlRequestSlot {
            load_content: 0xdead,
            file_cap: 0xbeef,
            ..latched
        };
        assert!(dispatch_sample_is_wedged(
            Some(compound),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        assert!(!save_request_is_orphaned(
            Some(compound),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        // Where the drain will act, the observer must have reported it first -- otherwise a
        // repaired state could exist that was never attributed to a frame.
        assert!(save_request_is_orphaned(
            Some(latched),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        assert!(dispatch_sample_is_wedged(
            Some(latched),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
    }

    /// The "never seen" reading must be a distinct third answer, not a zero that a reader can
    /// mistake for a measured value -- the same property `_load_poll_calls`/`_save_lane_calls`
    /// carry for the witness, restated for the fields that have no counter of their own.
    #[test]
    fn an_unobserved_first_latched_sample_reads_as_absent_not_as_zero() {
        assert_eq!(dispatch_first_latched_slot(), None);
        assert_eq!(dispatch_first_latched_call(), 0);
        assert_eq!(dispatch_first_latched_ms(), ELAPSED_MS_UNAVAILABLE);
        assert_eq!(dispatch_first_latched_lane(), SAVE_LANE_NONE);
        // And the plateau counter agrees: nothing observed, nothing counted.
        assert_eq!(dispatch_latched_declines(), 0);
    }

    /// With no clock sink wired the stamp must say so rather than invent a moment in time.
    #[test]
    fn an_unwired_clock_reports_the_sentinel_rather_than_a_time() {
        assert_eq!(elapsed_ms(), ELAPSED_MS_UNAVAILABLE);
    }
}
