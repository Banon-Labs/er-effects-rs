// WHO ABANDONED THE SAVE. The instrument that names the frame in which `GameMan.saveState` stops
// describing a save request the SL device is still holding.
//
// `include!`d into `lib.rs` like `save_orphan_drain.rs`, whose block above it establishes the state
// this one hunts the CAUSE of. That block ends at "once `saveState` leaves 1 with `+0x10` still
// populated, nothing will ever poll the device again"; everything here is about the word "leaves".
//
// WHAT STATIC ANALYSIS SETTLED, AND WHERE IT STOPPED (1.16.2 decompiles, shift 0). Exactly three
// writes can take `saveState` out of 1 while leaving `iodev+0x10` populated:
//
//  1. `FUN_140679180` -> `FUN_140e6e080` returns 4 on the spot when `iodev+0x18 == 0` (that field
//     belongs to a LOAD, so it is 0 for the whole life of a save) and releases nothing; the wrapper
//     then writes `saveState = 0` for any answer that is not 0 or 1. No other state is required, so
//     this is the one that needs no coincidence.
//  2. `FUN_140679180`'s own first branch writes `saveState = 3` when `GameMan+0xdd0` is a non-empty
//     string -- also without releasing. That leaves the orphan one write short, and needs a second
//     writer to bring 3 down to 0.
//  3. `FUN_140679510` -> `FUN_140e6e430` -> (`iodev+0x28 != 0`) `FUN_140e6f370` can return 9 after
//     clearing `+0x28` alone, and the wrapper writes `saveState = 0`. Narrower: it needs a deferred
//     file cap parked on the device and its enqueue to fail.
//
// Every OTHER non-1 answer from either poll passes through `FUN_140e6f200` first, so it clears
// `+0x10` on the way out and cannot produce the signature. That is as far as reading gets: all three
// are reachable, and which one fired is a fact about a particular frame, not about the code.
//
// SO THIS WITNESS ANSWERS THE REMAINING QUESTION AND NOTHING ELSE. It chains onto both wrappers,
// samples the device and `saveState` either side of the call, and reports the transitions that leave
// a save latched with nobody polling it -- with the caller's RVA, which is the part no decompile can
// supply, and which the decode table in `save_state_device.rs` turns into a function name.
//
// IT FORWARDS EVERY CALL UNCHANGED AND CHANGES NO RETURN VALUE. It writes game memory in exactly
// one place: on the LOAD-poll site it calls `restore_save_state_owner`, putting `saveState` back to
// the 1 the game held one instruction earlier, so the save it just lost goes back to the game's own
// pump. `save_state_device.rs` carries why that is a repair rather than a policy, and why the SAVE
// lane deliberately does NOT get the same treatment: its only non-releasing answer comes from
// `FUN_140e6f370`, which clears `iodev+0x28` on the way out, so a re-poll takes a DIFFERENT branch
// and the state it leaves is the orphan `drain_orphaned_save_request` is built for. Restoring there
// would hand a repaired-looking state to a repair that then cannot see it.
//
// WHY BOTH WRAPPERS AND NOT JUST THE LOAD POLL. Reporting only `FUN_140679180` would make case 3
// invisible, and an instrument that can only confirm the hypothesis it was built for is not
// evidence. Both are chained through the hook union, so the product's existing detour on
// `FUN_140679180` (the menu trace) keeps working and neither owner is silently dropped.

/// `FUN_140679180` (1.16.2): the LOAD-side poll wrapper. Writes `saveState` 3 or 0 depending on
/// `FUN_140e6e080`'s answer. 1.17 `0x140679fd0`, verdict IDENTICAL-WHOLE over 39 instructions,
/// BOTH-ENTRIES, PDATA:0x94/0x94 (`docs/recon/rva-map-1162-to-1170.needed-verified.tsv`).
const SAVE_STATE_LOAD_POLL_RVA: usize = er_game_base::rva::SL_LOAD_POLL_WRAPPER_RVA;

/// `FUN_140679510` (1.16.2): the SAVE-side lane wrapper. Writes `saveState = 0` for any
/// `FUN_140e6e430` answer that is not 1. 1.17 `0x14067a360`, IDENTICAL-WHOLE over 15 instructions,
/// BOTH-ENTRIES, 9B relocatable.
///
/// ITS `MOVSS XMM1` IS A DEAD STORE, checked rather than assumed. The wrapper loads a float into
/// XMM1 immediately before `call FUN_140e6e430`, which looks exactly like a float second argument
/// and would make the union's all-integer shape wrong for it. `FUN_140e6e430` reads ZERO XMM
/// registers -- 126 instructions on 1.17 (`0x140e70230`), none of them touching XMM, opening
/// `MOV RBX,RCX; CMP [RCX+0x10],0` on its single integer parameter. The same `MOVSS` is present on
/// 1.16.2 at `0x140679519`, so it is not migration drift either. The union shape is correct here.
const SAVE_STATE_SAVE_LANE_RVA: usize = er_game_base::rva::SL_SAVE_LANE_WRAPPER_RVA;

/// No abandoning write has been observed.
pub const SAVE_STATE_SITE_NONE: usize = 0;
/// The LOAD-side poll `FUN_140679180` was the writer.
pub const SAVE_STATE_SITE_LOAD_POLL: usize = 1;
/// The SAVE-side lane `FUN_140679510` was the writer.
pub const SAVE_STATE_SITE_SAVE_LANE: usize = 2;

/// Does an abandoning write at this site get `saveState` put back?
///
/// Only the LOAD poll. Pure and total so the scoping rule can be exercised with no game attached,
/// because it is a decision about which native branch is being undone, not about a runtime value:
///
/// * `FUN_140679180` reaches its `saveState = 0` through `FUN_140e6e080`'s `iodev+0x18 == 0`
///   early-out, which releases NOTHING and leaves the device byte-identical. Putting the 1 back
///   restores exactly the state the game had one instruction earlier, and the game's own pump
///   finishes the save from there.
/// * `FUN_140679510`'s only non-releasing answer comes from `FUN_140e6f370`, which UNLOADS the
///   file cap and zeroes `iodev+0x28` before returning 9. The device is no longer what it was, a
///   re-poll would take the `+0x28 == 0` branch instead, and the state left behind is precisely
///   the orphan `drain_orphaned_save_request` exists to release. Restoring there would hide it.
pub fn save_state_site_takes_the_repair(site: usize) -> bool {
    site == SAVE_STATE_SITE_LOAD_POLL
}

/// Name the site that abandoned a save.
pub fn save_state_site_label(site: usize) -> &'static str {
    match site {
        SAVE_STATE_SITE_LOAD_POLL => "load-poll-0x679180",
        SAVE_STATE_SITE_SAVE_LANE => "save-lane-0x679510",
        // The three writes `save_state_writers.rs` completes the set with. Named here rather than
        // there so one table answers "which site" for every site, and a new one cannot be added
        // without a reader noticing the others.
        SAVE_STATE_SITE_MENUJOB_LOADWAIT => "menujob-loadwait-0x678e00",
        SAVE_STATE_SITE_SAVE_LANE_ALT => "save-lane-alt-0x6794b0",
        SAVE_STATE_SITE_SETTER => "setsavestate-0x67ac90",
        _ => "none",
    }
}

/// Did this call take `saveState` off a save the device is still holding?
///
/// Pure and total so the rule that decides whether a log line is a real finding can be exercised on
/// the host. Each clause excludes a way of crying wolf:
///
/// * `before_state` must be `SAVE_OWNS`. If the game did not think a save was in flight when the
///   call started, this call did not take one away from it.
/// * `after_state` must have left `SAVE_OWNS`. Staying at 1 is the healthy in-flight answer and by
///   far the common case.
/// * the device must STILL hold the save afterwards. A normal completion goes through
///   `FUN_140e6f200` and comes back with `save_content` and `job` at zero -- that is the poll doing
///   its job, and reporting it would bury the real event in noise.
/// * an unreadable sample on either side proves nothing and is never a finding.
pub fn poll_abandoned_a_save(
    before_state: Option<u32>,
    after_state: Option<u32>,
    after: Option<SlRequestSlot>,
) -> bool {
    let (Some(before_state), Some(after_state), Some(after)) = (before_state, after_state, after)
    else {
        return false;
    };
    before_state == GAME_MAN_SAVE_STATE_SAVE_OWNS
        && after_state != GAME_MAN_SAVE_STATE_SAVE_OWNS
        && !after.admits_a_save()
}

/// Calls forwarded through each wrapper, so a run with zero findings can be told apart from a run
/// where the witness never installed.
static SAVE_STATE_LOAD_POLL_CALLS: AtomicU64 = AtomicU64::new(0);

/// One bit per distinct value the LOAD poll has returned (value mod 32), so the log carries each
/// branch once instead of once per frame. Answers above 31 fold onto a low bit; the poll's own
/// range is 0..=9, so no real answer collides.
static LOAD_POLL_ANSWERS_SEEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static SAVE_STATE_SAVE_LANE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Abandoning writes observed. **This is the finding.** Non-zero means saving died in this run and
/// the fields below say who did it.
static SAVE_STATE_ABANDONING_WRITES: AtomicU64 = AtomicU64::new(0);
/// `SAVE_STATE_SITE_*` of the first abandoning write. FIRST, not last: later calls re-observe a
/// device that is already latched, and the author of the state is the one that matters.
static SAVE_STATE_FIRST_SITE: AtomicUsize = AtomicUsize::new(SAVE_STATE_SITE_NONE);
/// Game RVA of the code that called the wrapper on that first abandoning write, or 0 when no caller
/// sink was wired. This is the value the whole module exists to produce.
static SAVE_STATE_FIRST_CALLER_RVA: AtomicUsize = AtomicUsize::new(0);
/// `saveState` immediately after that first abandoning write (0 and 3 are different stories: 3 needs
/// a second writer to finish the orphan, 0 completes it on the spot).
static SAVE_STATE_FIRST_AFTER: AtomicU32 = AtomicU32::new(u32::MAX);
/// The device as it stood after that first abandoning write.
static SAVE_STATE_FIRST_SLOT: SlotSampleCell = SlotSampleCell::new();

/// Where the caller's RVA comes from. The stack walk that produces it lives in the host DLL
/// (`crate::crashlog::trace_first_game_caller_rva`), like the log and publish sinks, because it is
/// the host that knows which module is the game.
pub type CallerRvaFn = fn() -> usize;
static CALLER_RVA_SINK: AtomicUsize = AtomicUsize::new(0);

/// Wire the caller-RVA source. Without it the witness still counts and still logs; it just cannot
/// name the caller, which is most of the point, so the log says so explicitly.
pub fn set_caller_rva_sink(sink: CallerRvaFn) {
    CALLER_RVA_SINK.store(sink as usize, Ordering::Release);
}

/// The caller's game RVA, or 0 when no sink was wired. Called ONLY on a finding: the stack walk is
/// far more expensive than the two samples, and a poll that behaved has nothing to attribute.
fn caller_rva() -> usize {
    let raw = CALLER_RVA_SINK.load(Ordering::Acquire);
    if raw == 0 {
        return 0;
    }
    let sink: CallerRvaFn = unsafe { core::mem::transmute::<usize, CallerRvaFn>(raw) };
    sink()
}

#[cfg(windows)]
static ORIG_SAVE_STATE_LOAD_POLL: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SAVE_STATE_SAVE_LANE: AtomicUsize = AtomicUsize::new(0);

/// Forward one wrapper call, sampling either side of it.
///
/// `orig` may be the game trampoline OR the next chained handler -- the union decides -- so it is
/// always called with the full four-argument shape.
///
/// # Safety
/// Installed only on the two audited wrappers above, whose parameters are integers.
#[cfg(windows)]
unsafe fn witness_call(
    orig: &'static AtomicUsize,
    site: usize,
    calls: &'static AtomicU64,
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    calls.fetch_add(1, Ordering::Relaxed);
    let raw = orig.load(Ordering::SeqCst);
    if raw == 0 {
        // Unreachable: the union publishes `orig` before it enables the detour. If it ever happens,
        // 1 is the only safe invention -- it is "still in flight" to BOTH wrappers, so neither
        // writes `saveState` and no caller mistakes it for a finished load.
        return 1;
    }
    let before_state = read_save_state();
    let original: UnionFn = unsafe { core::mem::transmute::<usize, UnionFn>(raw) };
    let answer = unsafe { original(a, b, c, d) };
    // WHICH ANSWER THE LOAD POLL GAVE, once per distinct value (bd er-effects-rs-1742). The whole
    // ProfileSummary boot fill hangs off this number: `STEP_LoadSaveData_Wait` (1.17 0x140af2d40)
    // fills the summary ONLY when this returns 3. Return 0 is the "done" path and advances WITHOUT
    // filling; 2 falls through to state 3 without filling; 4..9 are errors. The call COUNT was
    // already witnessed (4 calls on the parked run) but the counts cannot tell those branches
    // apart, and every attempt to infer the branch from a downstream consumer has been wrong.
    // One line per distinct value, so a poll the game runs every frame cannot flood the log.
    if site == SAVE_STATE_SITE_LOAD_POLL {
        let bit = 1u32 << (answer as u32 & 31);
        if LOAD_POLL_ANSWERS_SEEN.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
            log_message(format_args!(
                "save-state-witness: load poll 0x679180 returned {answer} for the FIRST time                  (saveState {before_state:?} -> {after_state_preview:?}); 3 is the only answer that                  fills ProfileSummary in STEP_LoadSaveData_Wait, 0 advances without filling",
                after_state_preview = read_save_state()
            ));
        }
    }
    let after_state = read_save_state();
    // Feed the whole-set counters in `save_state_writers.rs`. Recorded for EVERY witnessed call,
    // not only for a finding: a zero there is what says the write came from outside the set, and a
    // counter that only ticked on findings could never say that.
    note_writer_call(before_state, after_state);
    // Sampling the DEVICE is the expensive half -- a 0x38-byte read through a resolved global, on a
    // function the game polls every frame -- and the predicate cannot be true unless `saveState`
    // left `SAVE_OWNS`. So read it only then, and hand the predicate `None` otherwise: that is the
    // same answer it already gives for an unreadable sample, so the decision stays in one place
    // instead of being restated by the fast path.
    let after = (before_state == Some(GAME_MAN_SAVE_STATE_SAVE_OWNS) && after_state != before_state)
        .then(read_sl_slot)
        .flatten();
    if poll_abandoned_a_save(before_state, after_state, after) {
        note_abandoning_write(site, after_state, after, answer);
    }
    answer
}

/// Record and report one abandoning write. Split out so it is cold code the hot path only jumps to.
#[cfg(windows)]
fn note_abandoning_write(
    site: usize,
    after_state: Option<u32>,
    after: Option<SlRequestSlot>,
    answer: usize,
) {
    let total = SAVE_STATE_ABANDONING_WRITES.fetch_add(1, Ordering::SeqCst) + 1;
    let rva = caller_rva();
    // THE REPAIR, before the log line: the save is losable for as long as `saveState` is wrong, and
    // the next thing that runs on this thread is the caller, not us. LOAD-poll site only -- see the
    // header, and `save_state_device.rs` for why putting the game's own value back is not a policy.
    let repaired = save_state_site_takes_the_repair(site) && restore_save_state_owner();
    let first = SAVE_STATE_FIRST_SITE
        .compare_exchange(
            SAVE_STATE_SITE_NONE,
            site,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok();
    if first {
        SAVE_STATE_FIRST_CALLER_RVA.store(rva, Ordering::SeqCst);
        SAVE_STATE_FIRST_AFTER.store(after_state.unwrap_or(u32::MAX), Ordering::SeqCst);
        store_slot_sample(after, &SAVE_STATE_FIRST_SLOT);
    }
    // ALWAYS reported on the first occurrence: this is the event that ends saving for the process,
    // and a throttle that hid it behind a power-of-two rule would hide the only copy that matters.
    if should_report(total, first) {
        log_message(format_args!(
            "suppress: SAVE ABANDONED (#{total}) at {} -- the wrapper answered {answer} and left \
             GameMan.saveState at {} while the device still holds the save. Caller game RVA {} \
             (subtract 5 for the call site, then read the decode table in save_state_device.rs). \
             {}. Device after: {}",
            save_state_site_label(site),
            match after_state {
                Some(state) => state.to_string(),
                None => "unreadable".to_owned(),
            },
            match rva {
                0 => "unknown (no caller sink wired by the host DLL)".to_owned(),
                rva => format!("0x{rva:x}"),
            },
            if repaired {
                "saveState PUT BACK to 1, so the game's own pump finishes the save and runs \
                 `FUN_140e6f200`"
            } else if save_state_site_takes_the_repair(site) {
                "the repair could NOT be written (GameMan unaddressable), so nothing polls this \
                 save now and every later submit is refused"
            } else {
                "left for `drain_orphaned_save_request`: this site's non-releasing answer already \
                 cleared `iodev+0x28`, so a re-poll takes a different branch"
            },
            describe_slot(after)
        ));
        publish_snapshot();
    }
}

#[cfg(windows)]
unsafe extern "system" fn save_state_load_poll_witness(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    unsafe {
        witness_call(
            &ORIG_SAVE_STATE_LOAD_POLL,
            SAVE_STATE_SITE_LOAD_POLL,
            &SAVE_STATE_LOAD_POLL_CALLS,
            a,
            b,
            c,
            d,
        )
    }
}

#[cfg(windows)]
unsafe extern "system" fn save_state_save_lane_witness(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    unsafe {
        witness_call(
            &ORIG_SAVE_STATE_SAVE_LANE,
            SAVE_STATE_SITE_SAVE_LANE,
            &SAVE_STATE_SAVE_LANE_CALLS,
            a,
            b,
            c,
            d,
        )
    }
}

/// Chain both witnesses onto the wrappers. Never fatal: a failure costs attribution, not saving.
///
/// Registered through [`register_union_hook`], which resolves each 1.16.2 constant through the
/// detour-safe table -- the same audit `mh_install_hook_once` relies on -- and CHAINS rather than
/// competing with the product's existing detour on `FUN_140679180`.
#[cfg(windows)]
pub fn install_save_state_witness() {
    for (rva, handler, orig, name) in [
        (
            SAVE_STATE_LOAD_POLL_RVA,
            save_state_load_poll_witness as UnionFn,
            &ORIG_SAVE_STATE_LOAD_POLL,
            "load poll 0x679180",
        ),
        (
            SAVE_STATE_SAVE_LANE_RVA,
            save_state_save_lane_witness as UnionFn,
            &ORIG_SAVE_STATE_SAVE_LANE,
            "save lane 0x679510",
        ),
    ] {
        // NON-RESOLVING on purpose (`game_rva_for_hook`, not `game_rva`): `register_union_hook`
        // owns the single 1.16.2 -> 1.17 resolve. Resolving here as well would hand it an address
        // that can be both a destination of one row and the SOURCE of another, and the detour lands
        // on a third function in silence -- `scripts/check-double-resolved-hook-targets.py`.
        let Ok(address) = er_game_base::mem::game_rva_for_hook(rva as u32) else {
            log_message(format_args!(
                "suppress: save-state witness could not resolve {name}; an abandoned save will stay \
                 unattributed this run"
            ));
            continue;
        };
        match unsafe { register_union_hook(address, handler, orig) } {
            Ok(()) => log_message(format_args!(
                "suppress: save-state witness chained on {name} (0x{address:x}) -- observer only, \
                 forwards every call unchanged"
            )),
            Err(status) => log_message(format_args!(
                "suppress: save-state witness failed on {name}: {status:?}; an abandoned save will \
                 stay unattributed this run"
            )),
        }
    }
}

/// Calls forwarded through the LOAD-side poll wrapper. Zero means the witness never ran, which is a
/// different verdict from "no save was abandoned".
pub fn save_state_load_poll_calls() -> u64 {
    SAVE_STATE_LOAD_POLL_CALLS.load(Ordering::SeqCst)
}

/// Calls forwarded through the SAVE-side lane wrapper.
pub fn save_state_save_lane_calls() -> u64 {
    SAVE_STATE_SAVE_LANE_CALLS.load(Ordering::SeqCst)
}

/// Writes that left a save latched with nobody polling it. **Non-zero means saving died.**
pub fn save_state_abandoning_writes() -> u64 {
    SAVE_STATE_ABANDONING_WRITES.load(Ordering::SeqCst)
}

/// Name of the site that did it first.
pub fn save_state_first_site_label() -> &'static str {
    save_state_site_label(SAVE_STATE_FIRST_SITE.load(Ordering::SeqCst))
}

/// Game RVA of the code that called it, or 0 when unattributed.
pub fn save_state_first_caller_rva() -> usize {
    SAVE_STATE_FIRST_CALLER_RVA.load(Ordering::SeqCst)
}

/// `saveState` immediately after the first abandoning write (`u32::MAX` = never observed).
pub fn save_state_first_after() -> u32 {
    SAVE_STATE_FIRST_AFTER.load(Ordering::SeqCst)
}

/// The device as it stood after the first abandoning write.
pub fn save_state_first_slot() -> Option<SlRequestSlot> {
    SAVE_STATE_FIRST_SLOT.snapshot()
}

// The predicate is the whole reviewable surface of an instrument that otherwise only exists at
// runtime, so it is tested with no game attached -- same reason as `save_request_is_orphaned`.
#[cfg(test)]
mod save_state_witness_tests {
    use super::*;

    fn latched() -> SlRequestSlot {
        SlRequestSlot {
            // The measured 2026-08-31 signature.
            save_content: 0x9ad1_d280,
            job: 0x1_8e31_9ea0,
            ..SlRequestSlot::default()
        }
    }

    /// THE FINDING, and every way of not crying wolf about it.
    #[test]
    fn only_a_write_that_strands_a_held_save_is_a_finding() {
        let clear = SlRequestSlot::default();
        // saveState 1 -> 0 with the save still on the device: the event that ends saving.
        assert!(poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_IDLE),
            Some(latched())
        ));
        // 1 -> 3 stands the orphan up just as surely; it only needs one more writer to finish it.
        assert!(poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_LOAD_RESIDENT),
            Some(latched())
        ));
        // A HEALTHY completion: the poll's terminal arm ran `FUN_140e6f200`, so the device came
        // back empty. This is the common case and must never be reported.
        assert!(!poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_IDLE),
            Some(clear)
        ));
        // Still in flight -- the answer the wrapper gets on almost every frame of a real save.
        assert!(!poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(latched())
        ));
        // The game did not think a save was in flight when the call started, so this call took
        // nothing from it -- including the case where the device was ALREADY orphaned and later
        // polls keep seeing it. Attributing those to the wrong frame is the failure this excludes.
        assert!(!poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_IDLE),
            Some(GAME_MAN_SAVE_STATE_IDLE),
            Some(latched())
        ));
        // A load owning the device is not a save being abandoned.
        assert!(!poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_LOAD_OWNS),
            Some(GAME_MAN_SAVE_STATE_LOAD_RESIDENT),
            Some(clear)
        ));
        // Unreadable on either side proves nothing.
        assert!(!poll_abandoned_a_save(
            None,
            Some(GAME_MAN_SAVE_STATE_IDLE),
            Some(latched())
        ));
        assert!(!poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            None,
            Some(latched())
        ));
        assert!(!poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_IDLE),
            None
        ));
    }

    /// The repair is scoped by which native branch it undoes, not by which site is convenient.
    #[test]
    fn only_the_load_poll_site_gets_save_state_put_back() {
        assert!(save_state_site_takes_the_repair(SAVE_STATE_SITE_LOAD_POLL));
        // The save lane's non-releasing answer already unloaded the file cap; its leftovers are
        // the orphan the drain releases, and a restored `saveState` would hide them from it.
        assert!(!save_state_site_takes_the_repair(SAVE_STATE_SITE_SAVE_LANE));
        // "nothing happened" must never be treated as a reason to write game memory.
        assert!(!save_state_site_takes_the_repair(SAVE_STATE_SITE_NONE));
    }

    /// The two sites send a reader to two different functions, so a shared label would waste the
    /// one thing this instrument produces.
    #[test]
    fn every_witness_site_is_distinguishable() {
        let labels: Vec<&str> = [
            SAVE_STATE_SITE_NONE,
            SAVE_STATE_SITE_LOAD_POLL,
            SAVE_STATE_SITE_SAVE_LANE,
        ]
        .into_iter()
        .map(save_state_site_label)
        .collect();
        assert_eq!(labels.len(), 3);
        for (i, label) in labels.iter().enumerate() {
            assert!(!label.is_empty());
            assert!(!labels[..i].contains(label), "duplicate label {label}");
        }
    }

    /// The witness's own predicate and the orphan drain's must agree about the state they share:
    /// what this reports as abandoned is exactly what that will later find and repair.
    #[test]
    fn an_abandoned_save_is_the_orphan_the_drain_repairs() {
        let stranded = latched();
        assert!(poll_abandoned_a_save(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_IDLE),
            Some(stranded)
        ));
        assert!(save_request_is_orphaned(
            Some(stranded),
            Some(GAME_MAN_SAVE_STATE_IDLE)
        ));
    }
}
