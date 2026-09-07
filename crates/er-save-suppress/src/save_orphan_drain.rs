// The forgotten save request, and the repair that lets the game take it back.
//
// `include!`d into `lib.rs` so it shares the crate's flat module namespace (the same pattern
// `save_job_completion.rs` and `save_write_branch.rs` use) while keeping each file under the
// size gate.

// ============================================================================
// the orphaned save request. The state in which the game has forgotten a save it
// submitted, and therefore the state in which saving stops for the rest of the process.
//
// Measured (run br-20260831-160354-2513, after one System->Quit->Load Character reload):
// `oracle_save_dispatch_declines = 6177 of 6186 calls`, every decline carrying the same
// two pointers for four minutes --
//
//   +0x10 save_content=0x9ad1d280  +0x18 load_content=0x0  +0x20 job=0x18e319ea0
//   +0x28 file_cap=0x0             [save-content-and-job-latched-0x10+0x20]
//
// -- while `GameMan.saveState` read 0. That combination is UNREACHABLE while the game
// still owns the request, and it is permanent. The proof is three decompiled facts
// (1.16.2, shift 0):
//
//  1. `iodev+0x10`/`+0x20` are cleared by `FUN_140e6f200` and by nothing else.
//  2. On the save side the only path that reaches `FUN_140e6f200` is the poll
//     `FUN_140e6e430`, whose only two callers are `FUN_140679510` and `FUN_1406794b0`,
//     which are in turn reached only from `CS::MoveMapStep::DoSaveStuff` -- gated on
//     `GameMan::IsSaveState1()` -- and from the "saving..." MenuJob `FUN_14082a0f0`.
//  3. Every submit path sets `saveState = 1` at its commit tail (`FUN_14067b940`,
//     `FUN_14067b750`, `FUN_14067b570`, `FUN_14067bc10`), so `+0x10 != 0` and
//     `saveState == 1` are set together and cleared together.
//
// Once `saveState` leaves 1 with `+0x10` still populated, (2) says nothing will ever poll
// the device again, so (1) says the request is never released -- and every later submit
// builder (`FUN_140e6ef60`, `FUN_140e6ec70`) fails its opening
// `iodev+0x10 == 0 && iodev+0x20 == 0` guard. Autosave, rest, quit-save and the Save Game
// row are all refused, silently, forever. Elden Ring shows no error for a save that is
// never attempted, so the player keeps playing and loses everything they do.
//
// What this does about it. It runs the game's own poll, `FUN_140e6e430(iodev)`, once per
// refused dispatch that carries this signature. Nothing here decides whether the request
// may be freed: the poll's `case 0x14` arm frees it only when `FUN_14240a1f0(job)` reports
// terminal, which is the same guard `DoSaveStuff` would have applied on the frame the game
// forgot to. A job still in flight lands in a `break -> return 1` arm and nothing is
// touched. That is the same discipline the load side already follows in
// [`note_load_consumer`], and for the same reason: a guard we re-derive is a guard that can
// disagree with the game about whether a write is still running, and the object being freed
// is one the SL worker thread may still be writing through.
//
// Why it is safe to call from a decline. `FUN_140e6f200` frees both sides of the device
// (`+0x10`, `+0x18`, `+0x20`, `+0x28`), so a poll fired while a load owns the job would
// take that load's payload away before its consumer ever saw it. [`save_request_is_orphaned`]
// therefore demands the load side be empty and `saveState` be exactly idle -- which
// excludes a save in flight (1) and a load in any of its phases (2, 3, 7) -- and refuses to
// act on an unreadable sample. The decline itself already implies `saveState == 0`
// (`FUN_140afb880` picks no lane otherwise), so the check is a restatement the code can be
// read against rather than a new assumption.
//
// Why the first refused save is not lost. A declining lane touches nothing --
// `GameMan+0xb72`/`+0xb73` stay set -- so `FUN_140afb880` re-enters it on the next frame.
// The drain runs between those two entries, so the save the user was owed is built one
// frame later instead of never.
//
// What is deliberately not done. `FUN_140679510` would additionally retire
// `GameMan+0xbb8`/`+0xbc0` and write `saveState = 0`. That accounting is not replicated
// here: `saveState` is already 0 by the predicate above, and `+0xbb8` is read by that
// function alone -- the next genuine save's own poll retires it. Writing GameMan from an
// instrument to tidy a counter would be exactly the re-derivation this design refuses.
// ============================================================================

/// `GameMan+0xb80` IDLE: no save and no load owns the SL device.
pub const GAME_MAN_SAVE_STATE_IDLE: u32 = 0;

// ============================================================================
// `GameMan.saveState` is a MUTEX, not a progress bar -- and reading it as a progress bar is
// what killed saving after a Load Character reload in the first place.
//
// There is one SL device (the `FUN_140e6e060` singleton) and one `saveState` word arbitrating
// it. Every submit builder refuses unless it reads idle, and each stamps its own value on the
// way out (1.16.2, shift 0):
//
//   `FUN_14067b940` / `b750` / `b570` / `bc10`  save    -> saveState = 1, fills +0x10 and +0x20
//   `FUN_14067b4e0`                             preview -> saveState = 1, same two fields
//   `FUN_14067b1a0`                             load    -> saveState = 2, fills +0x18 and +0x20
//
// and `CS::MoveMapStep::DoSaveStuff` then picks exactly one pump by that value: `IsSaveState1()`
// -> `FUN_140679510` (which polls `FUN_140e6e430`, the save side), else `IsSaveState2()` ->
// `FUN_140679180` (which polls `FUN_140e6e080`, the load side). The two are never both run.
//
// They are not interchangeable, and the load-side one is actively destructive against a save:
// `FUN_140e6e080` opens with `if (iodev+0x18 == 0 || iodev+0x20 == 0) return 4;` -- and +0x18 is
// the load's buffer, so during a save it is 0 and the poll returns 4 having released nothing.
// `FUN_140679180` then writes `saveState = 0` for any answer that is not 0 or 1. That single
// write is the whole bug: `IsSaveState1()` goes false, `DoSaveStuff` stops polling the save,
// `FUN_140e6e430` -- the only save-side road to the release `FUN_140e6f200` -- is never called
// again, and +0x10/+0x20 stay latched for the life of the process. That is precisely the
// orphan the block above repairs, arriving with `saveState` already 0 exactly as measured.
//
// So these predicates exist to keep a caller from pumping a lane it does not own. They are pure
// and total for the same reason `save_request_is_orphaned` is: the decision guards a call into
// the engine that can cost the user every save they make afterwards.
// ============================================================================

/// `GameMan+0xb80` == 1: a save (`FUN_14067b940`/`b750`/`b570`/`bc10`) or the preview read
/// (`FUN_14067b4e0`) owns the device through `iodev+0x10` + `+0x20`. Pumped by `FUN_140679510` /
/// `FUN_1406794b0` alone.
pub const GAME_MAN_SAVE_STATE_SAVE_OWNS: u32 = 1;

/// `GameMan+0xb80` == 2: a load (`FUN_14067b1a0`) owns the device through `iodev+0x18` + `+0x20`.
/// This is the only value under which `FUN_140679180` may be called.
pub const GAME_MAN_SAVE_STATE_LOAD_OWNS: u32 = 2;

/// `GameMan+0xb80` == 3: the load's payload is resident. `FUN_140679180` reaches it by answering
/// 0, so a drain that polls after residency is polling a lane that is already finished.
pub const GAME_MAN_SAVE_STATE_LOAD_RESIDENT: u32 = 3;

/// May a submit builder be offered a request right now? Only idle means the device is unowned;
/// an unreadable sample is never treated as free.
pub fn sl_device_is_free(save_state: Option<u32>) -> bool {
    save_state == Some(GAME_MAN_SAVE_STATE_IDLE)
}

/// May the load-side poll `FUN_140679180` be run right now?
///
/// Only when a load we submitted actually owns the device. Every other answer is a refusal with
/// its own reason: Idle has no request to advance; `SAVE_OWNS` is the destructive case above;
/// `LOAD_RESIDENT` is already done; an unreadable sample proves nothing.
pub fn load_poll_may_run(save_state: Option<u32>) -> bool {
    save_state == Some(GAME_MAN_SAVE_STATE_LOAD_OWNS)
}

/// [`sl_device_is_free`] against the live `GameMan`. The read lives here, next to the rule, so a
/// caller cannot pass the wrong field or forget that an unreadable sample is not a free device.
#[cfg(windows)]
pub fn sl_device_is_free_now() -> bool {
    sl_device_is_free(read_save_state())
}

/// [`load_poll_may_run`] against the live `GameMan`. Same reason as above.
#[cfg(windows)]
pub fn load_poll_may_run_now() -> bool {
    load_poll_may_run(read_save_state())
}

/// No orphaned request was present, so nothing was attempted.
pub const SAVE_ORPHAN_NONE: usize = 0;
/// The device or `GameMan` could not be sampled; the state is unknown and untouched.
pub const SAVE_ORPHAN_UNREADABLE: usize = 1;
/// An orphan was present but the poll's address was never resolved, so it cannot be run.
/// The request stays latched and saving stays dead -- this is a loud failure, not a pass.
pub const SAVE_ORPHAN_POLL_UNAVAILABLE: usize = 2;
/// The game's poll ran and the request is gone: the submit builders' precondition is open.
pub const SAVE_ORPHAN_RELEASED: usize = 3;
/// The game's poll ran and kept the request, which is what it does while the job has not
/// reached its terminal state. Nothing was taken from a live write.
pub const SAVE_ORPHAN_STILL_LATCHED: usize = 4;

/// Name a [`drain_orphaned_save_request`] outcome.
pub fn save_orphan_outcome_label(code: usize) -> &'static str {
    match code {
        SAVE_ORPHAN_UNREADABLE => "iodev-or-gameman-unreadable",
        SAVE_ORPHAN_POLL_UNAVAILABLE => "poll-address-unresolved",
        SAVE_ORPHAN_RELEASED => "orphan-released",
        SAVE_ORPHAN_STILL_LATCHED => "orphan-still-latched",
        _ => "no-orphan",
    }
}

/// Does this device sample plus `saveState` describe a save request the game has forgotten?
///
/// Pure and total so the predicate that decides whether to call into the engine is
/// exercised on the host, with no game attached. Every arm is a refusal to act on
/// something that is not unambiguously an orphan:
///
/// * an unreadable device or `GameMan` proves nothing, so it is not an orphan;
/// * `save_content == 0` means there is no save request to release;
/// * `load_content != 0` or `file_cap != 0` means the load side or a deferred build also
///   owns this device, and `FUN_140e6f200` would free their objects too;
/// * `saveState != IDLE` means the game still owns the transaction -- 1 is a save in
///   flight that `DoSaveStuff` is polling, and 2/3/7 are load phases.
pub fn save_request_is_orphaned(slot: Option<SlRequestSlot>, save_state: Option<u32>) -> bool {
    let (Some(slot), Some(save_state)) = (slot, save_state) else {
        return false;
    };
    slot.save_content != 0
        && slot.load_content == 0
        && slot.file_cap == 0
        && save_state == GAME_MAN_SAVE_STATE_IDLE
}

/// `FUN_140e6e430`'s resolved, prologue-verified address for the running build.
///
/// Stored by both installers. Armed runs also detour this address, and the drain then
/// prefers the trampoline in [`ORIG_POLL_SAVE_STATUS`] so the repair cannot be re-read by
/// our own status rewrite on the way back out.
#[cfg(windows)]
static SL_POLL_SAVE_STATUS_ADDR: AtomicUsize = AtomicUsize::new(0);

/// Declines whose device sample matched [`save_request_is_orphaned`].
static SAVE_ORPHAN_DETECTIONS: AtomicU64 = AtomicU64::new(0);
/// Times the game's own poll was actually run against an orphan.
static SAVE_ORPHAN_DRAINS: AtomicU64 = AtomicU64::new(0);
/// Drains after which the submit precondition held again. This is the "saving works again"
/// oracle, and the only one of these counters whose non-zero value is good news.
static SAVE_ORPHAN_RELEASED_COUNT: AtomicU64 = AtomicU64::new(0);
/// Drains after which the request was still latched -- the native guard kept a job it does
/// not consider finished. Expected transiently; persistent non-zero means the job never
/// reaches terminal and the orphan has a different cause.
static SAVE_ORPHAN_STILL_LATCHED_COUNT: AtomicU64 = AtomicU64::new(0);
/// Declines that showed a latched save request the drain deliberately did not touch,
/// because the load side or a deferred `FD4FileCap` also owned the device. Non-zero means
/// saving is dead and this repair is not the one that fits.
static SAVE_ORPHAN_SHARED_DEVICE_SKIPS: AtomicU64 = AtomicU64::new(0);
/// Orphans found with no resolved poll address. Every one is an unrecoverable save system.
static SAVE_ORPHAN_POLL_UNAVAILABLE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Status the game's poll returned on the last drain (0 = the save had succeeded, 1 = still
/// in flight, 4 = no request, others are its failure codes).
static SAVE_ORPHAN_LAST_STATUS: AtomicU32 = AtomicU32::new(u32::MAX);
/// Outcome code of the most recent drain attempt.
static SAVE_ORPHAN_LAST_OUTCOME: AtomicUsize = AtomicUsize::new(SAVE_ORPHAN_NONE);
/// The device as it stood immediately before the last drain.
static SAVE_ORPHAN_SLOT_BEFORE: SlotSampleCell = SlotSampleCell::new();
/// The device as it stood immediately after it.
static SAVE_ORPHAN_SLOT_AFTER: SlotSampleCell = SlotSampleCell::new();

/// Detect a forgotten save request and, if there is one, let the game release it.
///
/// `origin` names the site that noticed, for the log. Returns the `SAVE_ORPHAN_*` outcome.
/// Callers do not have to act on it: the counters and the log line carry the verdict, and
/// the request that provoked the decline is retried by the dispatcher on the next frame.
#[cfg(windows)]
pub fn drain_orphaned_save_request(origin: &str) -> usize {
    let before = read_sl_slot();
    let save_state = read_save_state();
    if !save_request_is_orphaned(before, save_state) {
        // Separate the "a save IS latched but this device is shared" case from "nothing to
        // do": the first is still a dead save system, and reading it as a pass is the
        // failure mode this whole block exists to end.
        if before.is_some_and(|slot| !slot.admits_a_save())
            && save_state == Some(GAME_MAN_SAVE_STATE_IDLE)
        {
            let skips = SAVE_ORPHAN_SHARED_DEVICE_SKIPS.fetch_add(1, Ordering::SeqCst) + 1;
            if should_report(skips, false) {
                log_message(format_args!(
                    "suppress: {origin} found the submit precondition failing with saveState \
                     IDLE, but the LOAD side or a deferred file cap also owns the device \
                     (#{skips}) -- not draining, because the release frees their objects too. \
                     Slot: {}",
                    describe_slot(before)
                ));
                publish_snapshot();
            }
        }
        return SAVE_ORPHAN_NONE;
    }
    let detections = SAVE_ORPHAN_DETECTIONS.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(iodev) = read_sl_iodev() else {
        SAVE_ORPHAN_LAST_OUTCOME.store(SAVE_ORPHAN_UNREADABLE, Ordering::SeqCst);
        return SAVE_ORPHAN_UNREADABLE;
    };
    // The trampoline first: on an armed run this address carries our own status detour, and
    // going through it would re-enter `decide_status` on a poll nobody in the game asked for.
    let poll_address = match ORIG_POLL_SAVE_STATUS.load(Ordering::SeqCst) {
        0 => SL_POLL_SAVE_STATUS_ADDR.load(Ordering::SeqCst),
        trampoline => trampoline,
    };
    if poll_address == 0 {
        let unavailable = SAVE_ORPHAN_POLL_UNAVAILABLE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        SAVE_ORPHAN_LAST_OUTCOME.store(SAVE_ORPHAN_POLL_UNAVAILABLE, Ordering::SeqCst);
        if should_report(unavailable, false) {
            log_message(format_args!(
                "suppress: BUG -- {origin} found a FORGOTTEN save request (#{detections}) but \
                 `FUN_140e6e430` was never resolved for this build, so nothing can release it. \
                 Every save in this process stays refused. Slot: {}",
                describe_slot(before)
            ));
            publish_snapshot();
        }
        return SAVE_ORPHAN_POLL_UNAVAILABLE;
    }
    store_slot_sample(before, &SAVE_ORPHAN_SLOT_BEFORE);
    let poll: PollSaveStatusFn = unsafe { core::mem::transmute(poll_address) };
    let status = unsafe { poll(iodev) };
    let after = read_sl_slot();
    store_slot_sample(after, &SAVE_ORPHAN_SLOT_AFTER);
    SAVE_ORPHAN_LAST_STATUS.store(status, Ordering::SeqCst);
    let drains = SAVE_ORPHAN_DRAINS.fetch_add(1, Ordering::SeqCst) + 1;
    let outcome = if after.is_some_and(|slot| slot.admits_a_save()) {
        SAVE_ORPHAN_RELEASED
    } else {
        SAVE_ORPHAN_STILL_LATCHED
    };
    SAVE_ORPHAN_LAST_OUTCOME.store(outcome, Ordering::SeqCst);
    if outcome == SAVE_ORPHAN_RELEASED {
        let released = SAVE_ORPHAN_RELEASED_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if should_report(released, false) {
            log_message(format_args!(
                "suppress: {origin} released a FORGOTTEN save request (release #{released} of \
                 {drains} drains, {detections} detections) -- the game's own poll \
                 `FUN_140e6e430` returned status {status} (0 = the write had succeeded) and its \
                 terminal arm ran `FUN_140e6f200`. Before {}, after {}. The submit builders' \
                 `iodev+0x10 == 0 && iodev+0x20 == 0` precondition is open again, so the \
                 request the dispatcher is still holding builds on the next frame and saving \
                 works for the rest of this process",
                describe_slot(before),
                describe_slot(after)
            ));
            publish_snapshot();
        }
    } else {
        let still = SAVE_ORPHAN_STILL_LATCHED_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if should_report(still, false) {
            log_message(format_args!(
                "suppress: {origin} ran the game's poll on a forgotten save request and it kept \
                 it (#{still} of {drains} drains) -- status {status}, so the job at iodev+0x20 \
                 has not reached its terminal state and nothing was taken from a live write. \
                 Before {}, after {}",
                describe_slot(before),
                describe_slot(after)
            ));
            publish_snapshot();
        }
    }
    outcome
}

/// Declines whose device sample was a forgotten save request.
pub fn save_orphan_detections() -> u64 {
    SAVE_ORPHAN_DETECTIONS.load(Ordering::SeqCst)
}

/// Times the game's own poll was run against one.
pub fn save_orphan_drains() -> u64 {
    SAVE_ORPHAN_DRAINS.load(Ordering::SeqCst)
}

/// Drains that freed the request. **This is the oracle that says saving survived a
/// reload**: a run with `save_orphan_detections > 0` and this at 0 lost the save system.
pub fn save_orphan_released() -> u64 {
    SAVE_ORPHAN_RELEASED_COUNT.load(Ordering::SeqCst)
}

/// Drains after which the native guard still held the job.
pub fn save_orphan_still_latched() -> u64 {
    SAVE_ORPHAN_STILL_LATCHED_COUNT.load(Ordering::SeqCst)
}

/// Refused saves whose device was shared with the load side, so the drain stood down.
pub fn save_orphan_shared_device_skips() -> u64 {
    SAVE_ORPHAN_SHARED_DEVICE_SKIPS.load(Ordering::SeqCst)
}

/// Orphans found with no resolvable poll. Every one is a save system that cannot recover.
pub fn save_orphan_poll_unavailable() -> u64 {
    SAVE_ORPHAN_POLL_UNAVAILABLE_COUNT.load(Ordering::SeqCst)
}

/// Status the game's poll returned on the last drain (`u32::MAX` = never drained).
pub fn save_orphan_last_status() -> u32 {
    SAVE_ORPHAN_LAST_STATUS.load(Ordering::SeqCst)
}

/// Name the most recent drain outcome.
pub fn save_orphan_last_outcome_label() -> &'static str {
    save_orphan_outcome_label(SAVE_ORPHAN_LAST_OUTCOME.load(Ordering::SeqCst))
}

/// The device as it stood before the last drain.
pub fn save_orphan_slot_before() -> Option<SlRequestSlot> {
    SAVE_ORPHAN_SLOT_BEFORE.snapshot()
}

/// The device as it stood after the last drain.
pub fn save_orphan_slot_after() -> Option<SlRequestSlot> {
    SAVE_ORPHAN_SLOT_AFTER.snapshot()
}

// ============================================================================
// the other consumer of the same device, and the reason a repair that only runs on a
// declined save is not a repair at all.
//
// `iodev+0x20` is shared (see `SlRequestSlot::admits_a_load`), so the orphan above does not
// merely kill saving -- it kills loading, through a completely different door. The load
// builder (`FUN_140e6f430`/`FUN_140e6f5b0`, reached from `FUN_14067b1a0`) opens with
// `iodev+0x18 == 0 && iodev+0x20 == 0` on every branch and returns 0 without releasing
// anything. Its caller then leaves `GameMan+0xb80` at idle, so a caller that armed a drain
// on the strength of `saveState == 0` waits for `b80 == RESIDENT(3)` on a request that was
// never accepted. That wait is not slow, it is EMPTY: unsatisfiable by construction.
//
// Measured (run claims76-20260831-142135, the third same-character load): two submit lines
// 18s apart, same format string --
//
//   [+32999ms] reload-fd4io: Submit slot=1 ... ret=1 b80=2 -> drain   commits, load2 renders
//   [+51260ms] reload-fd4io: Submit slot=1 ... ret=0 b80=0 -> drain   b80 pinned at 0, 281 waits
//
// -- with an orphaned save latched onto the device 6 seconds before the second one.
//
// Why `sl_device_is_free` alone could not see it. That predicate reads `GameMan+0xb80` and
// nothing else, and the wedge's defining property is `saveState == 0` with the device
// latched. The mutex says the device is unowned while the device itself is occupied, so a
// mutex-only gate is structurally blind to exactly the state that refuses. Both operands
// have to be read, and `SlRequestSlot::admits_a_load` is the one that reads the second.
//
// Where the repair is driven from, and why it is not a private tick. The orphan is created
// by the last save of a world: `CS::MoveMapStep::DoSaveStuff` polls only under
// `IsSaveState1()`, so once `saveState` falls back to 0 the game's own completion owner is
// disarmed by its own selector and nothing will ever poll again. Until now the only site
// that noticed was `observe_dispatch`'s decline arm -- which requires a later save to be
// requested. When the wedge is the last save of a world, no later save comes, and the repair
// that exists is unreachable.
//
// The fix is not a timer and not a per-frame sweep. It is the other point of use: the device
// has exactly two consumers, and each one now checks the precondition it is about to be
// judged against, at the moment it is about to submit, and asks the game to release what it
// finds. The save lane checks it in `observe_dispatch` (inside `FUN_140afb880`, the game's
// own dispatcher, on the game's own frame); the load lane checks it here, called from the
// switch reload's submit gate. Neither invents a schedule, neither writes a device field,
// and the only thing that frees anything is still `FUN_140e6f200` reached through the game's
// own `FUN_140e6e430` terminal arm. A wedge with neither a later save nor a later load harms
// nothing until one of the two arrives -- and both arrivals now repair it.
// ============================================================================

/// The device admitted a load with nothing to repair.
pub const LOAD_GATE_OPEN: usize = 0;
/// `GameMan.saveState` is not idle, so a submit builder refuses before it ever reads the
/// device. Nothing here may hurry that along: the owning transaction is live and the fields
/// belong to it.
pub const LOAD_GATE_MUTEX_HELD: usize = 1;
/// The device could not be sampled. Never reported as admitting -- an unreadable sample is
/// the one answer that proves nothing.
pub const LOAD_GATE_UNREADABLE: usize = 2;
/// The device refused a load, and the game's own poll released it on this call.
pub const LOAD_GATE_REPAIRED: usize = 3;
/// The device refused a load and still refuses after the repair attempt. Either the job has
/// not gone terminal yet (retry next frame) or the holder is not a drainable orphan.
pub const LOAD_GATE_STILL_HELD: usize = 4;

/// Name a [`LoadGate`] outcome for a log line.
pub fn load_gate_outcome_label(code: usize) -> &'static str {
    match code {
        LOAD_GATE_OPEN => "open",
        LOAD_GATE_MUTEX_HELD => "mutex-held",
        LOAD_GATE_UNREADABLE => "iodev-unreadable",
        LOAD_GATE_REPAIRED => "repaired",
        LOAD_GATE_STILL_HELD => "still-held",
        _ => "unknown",
    }
}

/// What the load-submit gate saw, and what it did about it.
#[derive(Clone, Copy, Debug)]
pub struct LoadGate {
    /// One of the `LOAD_GATE_*` codes.
    pub outcome: usize,
    /// The device as the gate first read it.
    pub before: Option<SlRequestSlot>,
    /// The device after any repair. Equal to `before` when none was attempted.
    pub after: Option<SlRequestSlot>,
    /// `GameMan+0xb80` at the gate.
    pub save_state: Option<u32>,
}

impl LoadGate {
    /// May the full-read initiator be offered a request right now?
    ///
    /// Only the two outcomes that actually satisfy `iodev+0x18 == 0 && iodev+0x20 == 0`
    /// with the mutex idle. An unreadable sample is a refusal, for the same reason
    /// [`sl_device_is_free`] treats it as one.
    pub fn admits(&self) -> bool {
        matches!(self.outcome, LOAD_GATE_OPEN | LOAD_GATE_REPAIRED)
    }

    /// The gate as one log fragment: the verdict plus every operand behind it.
    pub fn describe(&self) -> String {
        let state = match self.save_state {
            Some(state) => format!("{state}"),
            None => "unreadable".to_owned(),
        };
        if self.outcome == LOAD_GATE_OPEN || self.outcome == LOAD_GATE_MUTEX_HELD {
            return format!(
                "{} saveState={state} device {}",
                load_gate_outcome_label(self.outcome),
                describe_slot(self.before)
            );
        }
        format!(
            "{} saveState={state} device before {} after {}",
            load_gate_outcome_label(self.outcome),
            describe_slot(self.before),
            describe_slot(self.after)
        )
    }
}

/// Gate calls whose device refused a load, i.e. the state the old mutex-only gate could not see.
static LOAD_GATE_REFUSALS: AtomicU64 = AtomicU64::new(0);
/// Gate calls after which the device admitted a load again because the game's poll released
/// the orphan. **This is the "the third load was not blocked by a forgotten save" oracle.**
static LOAD_GATE_REPAIRS: AtomicU64 = AtomicU64::new(0);

/// Read the load builder's real precondition, and repair it if a forgotten save is what fails it.
///
/// `origin` names the calling submit site for the log. Pure observation plus, at most, one call
/// to [`drain_orphaned_save_request`] -- which refuses everything that is not unambiguously an
/// orphan and frees nothing itself. The device is re-read after the repair so a release that
/// happened on this call is usable on this frame rather than the next.
#[cfg(windows)]
pub fn open_the_device_for_a_load(origin: &str) -> LoadGate {
    let save_state = read_save_state();
    let before = read_sl_slot();
    if !sl_device_is_free(save_state) {
        // A live transaction owns the mutex. It is not an orphan and it is not ours.
        return LoadGate {
            outcome: LOAD_GATE_MUTEX_HELD,
            before,
            after: before,
            save_state,
        };
    }
    let Some(sample) = before else {
        return LoadGate {
            outcome: LOAD_GATE_UNREADABLE,
            before,
            after: before,
            save_state,
        };
    };
    if sample.admits_a_load() {
        return LoadGate {
            outcome: LOAD_GATE_OPEN,
            before,
            after: before,
            save_state,
        };
    }
    let refusals = LOAD_GATE_REFUSALS.fetch_add(1, Ordering::SeqCst) + 1;
    drain_orphaned_save_request(origin);
    let after = read_sl_slot();
    let outcome = if after.is_some_and(|slot| slot.admits_a_load()) {
        let repairs = LOAD_GATE_REPAIRS.fetch_add(1, Ordering::SeqCst) + 1;
        if should_report(repairs, false) {
            log_message(format_args!(
                "suppress: {origin} found the LOAD builder's `iodev+0x18 == 0 && \
                 iodev+0x20 == 0` precondition failing with saveState IDLE (refusal \
                 #{refusals}) and the game's own poll released it (repair #{repairs}). \
                 Before {}, after {}. The full-read initiator can be offered a request \
                 again, so the reload submits instead of arming a drain on a request that \
                 was never accepted",
                describe_slot(before),
                describe_slot(after)
            ));
            publish_snapshot();
        }
        LOAD_GATE_REPAIRED
    } else {
        if should_report(refusals, false) {
            log_message(format_args!(
                "suppress: {origin} found the LOAD builder's precondition failing with \
                 saveState IDLE (refusal #{refusals}) and it is STILL failing after the \
                 repair attempt -- the full-read initiator would return 0 and the caller \
                 must NOT arm a drain. Before {}, after {}",
                describe_slot(before),
                describe_slot(after)
            ));
            publish_snapshot();
        }
        LOAD_GATE_STILL_HELD
    };
    LoadGate {
        outcome,
        before,
        after,
        save_state,
    }
}

/// The full-read initiator's verdict as one log fragment: its answer, the mutex it left behind,
/// what the caller's gate had concluded, and the device on both sides of the call.
///
/// `ret == 0` means the builder refused and no request exists, so `b80` can never leave 0 --
/// the sentence says that outright rather than leaving a reader to infer it from two numbers.
/// It lives here because every operand in it is this crate's model of the SL device.
pub fn describe_load_submit(
    ret: i32,
    save_state: i32,
    gate: &str,
    before: Option<SlRequestSlot>,
    after: Option<SlRequestSlot>,
) -> String {
    format!(
        "ret={ret} b80={save_state} gate={gate} accepted={} -> {} | device before {} | device after {}",
        ret != 0,
        if ret != 0 {
            "DRAIN (replicating boot native-fullread residency before feed+continue_confirm)"
        } else {
            "NO DRAIN (the initiator refused, so b80 can never leave 0 and the wait would be empty)"
        },
        describe_slot(before),
        describe_slot(after)
    )
}

/// Load-submit gate calls whose device refused the load builder's precondition.
pub fn load_gate_refusals() -> u64 {
    LOAD_GATE_REFUSALS.load(Ordering::SeqCst)
}

/// Refusals the game's own poll cleared. `refusals > 0` with this at 0 is a load the device
/// blocked and nothing could unblock.
pub fn load_gate_repairs() -> u64 {
    LOAD_GATE_REPAIRS.load(Ordering::SeqCst)
}

// The predicate is pure and portable on PURPOSE: it is what decides whether to free an object
// the SL worker thread may still be writing the user's save through, so it must be checkable
// with no game attached. No `cfg(windows)` here -- that would put it back out of reach of
// `cargo test`, which is where the crate's unit tests had spent their whole life.
#[cfg(test)]
mod save_orphan_drain_tests {
    use super::*;

    /// The PREDICATE that decides whether to call into the engine, exercised with no game
    /// attached. Each refusal here is a way the drain could have freed an object the game
    /// was still using, so each gets its own case rather than being folded into one.
    #[test]
    fn only_an_unambiguously_forgotten_save_request_is_drained() {
        let clear = SlRequestSlot::default();
        let orphan = SlRequestSlot {
            // The measured 2026-08-31 signature: a save's content and job, no load side.
            save_content: 0x9ad1_d280,
            job: 0x1_8e31_9ea0,
            ..clear
        };
        // The state this exists for, and the only one that is drained.
        assert!(save_request_is_orphaned(
            Some(orphan),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        // A save the game is still polling. Freeing this races the SL worker's own write.
        assert!(!save_request_is_orphaned(Some(orphan), Some(1)));
        // Load phases (reading / resident / the 7 the quit path uses). `FUN_140e6f200`
        // frees both sides, so a poll fired here takes the load's payload away too.
        for load_phase in [2, 3, 7] {
            assert!(
                !save_request_is_orphaned(Some(orphan), Some(load_phase)),
                "saveState {load_phase} still belongs to a load"
            );
        }
        // Nothing to release.
        assert!(!save_request_is_orphaned(
            Some(clear),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        // The load side owns the device as well -- the shared-device skip, not a drain.
        assert!(!save_request_is_orphaned(
            Some(SlRequestSlot {
                load_content: 0xb368_6200,
                ..orphan
            }),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        // A deferred build is parked on an `FD4FileCap` the release would unload.
        assert!(!save_request_is_orphaned(
            Some(SlRequestSlot {
                file_cap: 0x1234_5678,
                ..orphan
            }),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        // An unreadable sample proves nothing, on either side.
        assert!(!save_request_is_orphaned(
            None,
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        assert!(!save_request_is_orphaned(Some(orphan), None));
    }

    /// The write that creates the orphan, stated as a rule a caller can be held to.
    ///
    /// `FUN_140679180` writes `saveState = 0` for any poll answer that is not 0 or 1, and
    /// `FUN_140e6e080` answers 4 on the spot whenever `iodev+0x18 == 0` -- which is every frame
    /// a save owns the device. So running the load-side poll under `SAVE_OWNS` converts a live
    /// save into exactly the `save_request_is_orphaned` signature above, permanently. The
    /// predicate must therefore refuse that state, and refuse the two states with nothing to
    /// advance, and refuse an unreadable sample.
    #[test]
    fn the_load_poll_runs_only_while_a_load_owns_the_device() {
        assert!(load_poll_may_run(Some(GAME_MAN_SAVE_STATE_LOAD_OWNS)));
        // The destructive case: this is the write that wedged saving for the whole process.
        assert!(!load_poll_may_run(Some(GAME_MAN_SAVE_STATE_SAVE_OWNS)));
        // Nothing submitted, so nothing to advance -- and the poll would still answer 4.
        assert!(!load_poll_may_run(Some(GAME_MAN_SAVE_STATE_IDLE)));
        // Already resident; polling again re-enters a finished lane.
        assert!(!load_poll_may_run(Some(GAME_MAN_SAVE_STATE_LOAD_RESIDENT)));
        assert!(!load_poll_may_run(None));
        // Any value the build might invent stays a refusal rather than a default-allow.
        for unknown in [4, 7, 9, u32::MAX] {
            assert!(!load_poll_may_run(Some(unknown)), "saveState {unknown}");
        }
    }

    /// A submit offered to an owned device is refused by the game anyway (`FUN_14067b1a0` and
    /// every save builder open with a `saveState == 0` test), so the caller's own gate has to
    /// agree with that test exactly -- including treating an unreadable sample as not-free.
    #[test]
    fn the_device_is_offered_a_request_only_when_it_is_idle() {
        assert!(sl_device_is_free(Some(GAME_MAN_SAVE_STATE_IDLE)));
        assert!(!sl_device_is_free(Some(GAME_MAN_SAVE_STATE_SAVE_OWNS)));
        assert!(!sl_device_is_free(Some(GAME_MAN_SAVE_STATE_LOAD_OWNS)));
        assert!(!sl_device_is_free(Some(GAME_MAN_SAVE_STATE_LOAD_RESIDENT)));
        assert!(!sl_device_is_free(None));
    }

    /// The two predicates name disjoint states, and neither overlaps the orphan the drain
    /// repairs: an orphan is idle (so the load poll stands down) yet the device is not free for
    /// a new request until the drain releases it -- which is why `sl_device_is_free` alone is
    /// not a safe submit gate and the orphan predicate exists separately.
    #[test]
    fn the_owner_states_do_not_overlap() {
        for state in [
            GAME_MAN_SAVE_STATE_IDLE,
            GAME_MAN_SAVE_STATE_SAVE_OWNS,
            GAME_MAN_SAVE_STATE_LOAD_OWNS,
            GAME_MAN_SAVE_STATE_LOAD_RESIDENT,
        ] {
            assert!(
                !(sl_device_is_free(Some(state)) && load_poll_may_run(Some(state))),
                "saveState {state} claimed by both predicates"
            );
        }
        let orphan = SlRequestSlot {
            save_content: 0x9ad1_d280,
            job: 0x1_8e31_9ea0,
            ..SlRequestSlot::default()
        };
        assert!(save_request_is_orphaned(
            Some(orphan),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
        assert!(sl_device_is_free(Some(GAME_MAN_SAVE_STATE_IDLE)));
        assert!(!orphan.admits_a_save());
    }

    /// The PREDICATE the third same-character load hung on, exercised with no game attached.
    ///
    /// The wedge is `saveState == 0` with the device latched, so a gate that reads only the
    /// mutex reports "free" on the exact state the load builder refuses. These cases pin the
    /// two apart: `sl_device_is_free` says yes to all of them, `admits` says yes to none of
    /// the held ones. An unreadable device is a refusal, because a submit offered on the
    /// strength of an unknown returns 0 and arms an unsatisfiable drain.
    #[test]
    fn a_latched_device_never_admits_a_load_however_idle_the_mutex_is() {
        let wedge = SlRequestSlot {
            // The measured 2026-08-31 third-load signature.
            save_content: 0x8875_0080,
            job: 0x8848_6d80,
            ..SlRequestSlot::default()
        };
        let gate = |outcome, before| LoadGate {
            outcome,
            before,
            after: before,
            save_state: Some(GAME_MAN_SAVE_STATE_IDLE),
        };
        // The mutex is idle for every case below, which is the whole trap.
        assert!(sl_device_is_free(Some(GAME_MAN_SAVE_STATE_IDLE)));
        assert!(!wedge.admits_a_load());
        assert!(!gate(LOAD_GATE_STILL_HELD, Some(wedge)).admits());
        assert!(!gate(LOAD_GATE_UNREADABLE, None).admits());
        assert!(!gate(LOAD_GATE_MUTEX_HELD, Some(wedge)).admits());
        // The two that genuinely satisfy `iodev+0x18 == 0 && iodev+0x20 == 0`.
        assert!(gate(LOAD_GATE_OPEN, Some(SlRequestSlot::default())).admits());
        assert!(gate(LOAD_GATE_REPAIRED, Some(SlRequestSlot::default())).admits());
        // A load holding the job is refused just as firmly as a stale save: `+0x20` is shared.
        assert!(
            !SlRequestSlot {
                load_content: 0xb368_6200,
                job: 0x8848_6d80,
                ..SlRequestSlot::default()
            }
            .admits_a_load()
        );
        // Every outcome names itself, and the describe never panics on an unreadable side.
        let mut labels = Vec::new();
        for code in [
            LOAD_GATE_OPEN,
            LOAD_GATE_MUTEX_HELD,
            LOAD_GATE_UNREADABLE,
            LOAD_GATE_REPAIRED,
            LOAD_GATE_STILL_HELD,
        ] {
            let label = load_gate_outcome_label(code);
            assert!(!labels.contains(&label), "duplicate label {label}");
            labels.push(label);
            assert!(gate(code, None).describe().contains(label));
        }
    }

    /// Every drain outcome needs its own code and its own name: `save_orphan_released` and
    /// `save_orphan_still_latched` are opposite verdicts about the user's save system, and a
    /// shared label would let a run that lost saving read like one that recovered.
    #[test]
    fn every_save_orphan_outcome_is_distinguishable() {
        let codes = [
            SAVE_ORPHAN_NONE,
            SAVE_ORPHAN_UNREADABLE,
            SAVE_ORPHAN_POLL_UNAVAILABLE,
            SAVE_ORPHAN_RELEASED,
            SAVE_ORPHAN_STILL_LATCHED,
        ];
        let mut labels = Vec::new();
        for code in codes {
            let label = save_orphan_outcome_label(code);
            assert!(!label.is_empty());
            assert!(!labels.contains(&label), "duplicate label {label}");
            labels.push(label);
        }
        assert_eq!(labels.len(), codes.len());
    }
}
