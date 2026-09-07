// The rest of the `saveState` writer set -- the three writes `save_state_witness.rs` could not see,
// and the counter that says when the answer is "none of them".
//
// `include!`d into `lib.rs` like the blocks around it. `save_state_witness.rs` chains observers onto
// the two wrappers that were reachable when it was written; this file completes the set, using the
// same mechanism (a union-chained observer that samples `saveState` and the SL device either side of
// the call, forwards every argument and changes no return value). It adds no new kind of instrument.
//
// Why it exists. The 2026-08-31 wedge is not a stranded save: at the instant `saveState` went 1 -> 0
// the game's own poll `FUN_140e6e430` answered status 1 on the latched request, and 250 ms later the
// same poll answered 0 and released the device. So the write landed while the SL worker was still
// writing, and the two witnessed wrappers reported `abandoning_writes = 0` in the very run that
// wedged. The remaining writes had to be enumerated from the image rather than guessed at.
//
// The complete writer set, byte-scanned on both images and paired (`mov [reg+0xb80], imm/reg`, every
// store form including the register one an immediate-only scan misses). 22 stores in each image,
// pairing exactly at +0xE50 with no 1.17-only writer and none missing -- so the 1.17 game this branch
// targets has the same writers as the 1.16.2 decompiles below describe:
//
//   1.16.2      1.17        function                who can reach it                       witnessed
//   0x677f5d    0x678dad    FUN_140677f40           DoSaveStuff under `IsSaveState7`        no (*)
//   0x678e28    0x679c78    FUN_140678e00           MenuJob step FUN_14082a450              this file
//   0x6791a2..  0x679ff2..  FUN_140679180           load poll (3 and 0)                     witness
//   0x6794ee    0x67a33e    FUN_1406794b0           MenuJob step FUN_14082a0f0              this file
//   0x67954b    0x67a39b    FUN_140679510           DoSaveStuff / FUN_14082a0f0             witness
//   0x67ac97    0x67bae7    FUN_14067ac90           `SetSaveState(int)` -- see below        this file
//   0x67b08f..  0x67bedf..  submit builders         each guarded on `saveState == 0`        n/a
//   0x67b125/61 0x67bf75/b1 FUN_14067b100           guarded on `saveState == 3`             n/a
//
//   (*) `FUN_140677f40` writes `saveState = 0` unconditionally, but `CS::MoveMapStep::DoSaveStuff`
//       reaches it only through `IsSaveState7`, and the wedge forms out of `saveState == 1`, where
//       `DoSaveStuff` takes the `IsSaveState1` arm instead. The three arms are mutually exclusive
//       in one `if/else if/else`, so this is exclusion by the caller's own structure.
//
// The one that is not a poll wrapper, and the reason this file is worth the hooks:
// `FUN_14067ac90` is `GameMan::SetSaveState(int)` -- 14 bytes, `MOV RAX,[0x143d69918];
// MOV [RAX+0xb80],ECX; RET`. It is the only store to the field that writes a register rather than an
// immediate, and its one caller in the whole image is `FUN_140aff640`, the per-frame in-map
// `MoveMapStep` tick, three statements after `DoSaveStuff` and `FUN_140afb880`:
//
//     MOVSS  XMM6,[RDI+0x8]            ; the frame delta
//     call   0x14067a080               ; IsSaveStateIdle()
//     test   AL,AL / JNZ  ...          ; idle -> skip
//     ADDSS  XMM6,[RBX+0x130]          ; accumulate, only while a transaction owns the device
//     COMISS XMM6,[0x142b60870]        ; == 210.0f
//     MOVSS  [RBX+0x130],XMM6
//     JBE    ...
//     MOV    [RBX+0x130],ESI           ; acc = 0
//     call   0x14067ac90               ; SetSaveState(0)   <-- releases nothing
//
// That is a WATCHDOG: 210 seconds of continuously non-idle `saveState` and the field is forced to 0
// with the SL device untouched, which is the wedge signature exactly. The accumulator at
// `MoveMapStep+0x130` is zeroed only inside `FUN_140afb880`, which returns early at its own
// `IsSaveStateIdle` gate -- so nothing can reset it while a transaction is in flight and the deadline
// is unconditional once the clock starts.
//
// It is not the 2026-08-31 writer, and saying so is the point of measuring rather than assuming: that
// run reached the world at +59230 ms and the wedge was first visible at +84123 ms, so at most ~25 s
// could have accumulated against a 210 s threshold. The hook is here because a writer nobody had
// named is worth an oracle whatever this run says, and because a run that does exceed 210 s (a long
// session, a stalled worker) has no other way to tell this apart from every other cause.
//
// And the counter that makes a negative worth something. With these three the witnessed set covers
// every store in the image that can take `saveState` off 1. `writer_state_exits` counts, across all
// witnessed sites, the calls that observed `saveState` leave `SAVE_OWNS` -- device irrelevant, so it
// cannot be argued away by the device sample. If a wedge is born with that count at zero and the
// sites installed, the write did not come through any of them, and the next thing to doubt is the
// model (the field, the singleton, our own writers) rather than another arm of another poll.

/// `FUN_140678e00` (1.16.2) -- the MenuJob load-save-data wait step's poll wrapper. Calls
/// `FUN_140e6de10` on the SL device and writes `saveState = 0` for any answer that is not 1. Its one
/// caller is `FUN_14082a450`, a `MenuJobResult` step, which is exactly the pipeline the 2026-08-31
/// wedge window contains. 1.17 `0x140679c50`, identical-whole over 14 instructions, both-entries.
const SAVE_STATE_MENUJOB_LOADWAIT_RVA: usize = 0x678e00;

/// `FUN_1406794b0` (1.16.2) -- the alternate save-wait wrapper, and the reason the original witness
/// could miss a save-side write entirely. Its one caller `FUN_14082a0f0` picks between it and
/// `FUN_140679510` on a float at `MenuJob+0x8`:
///
/// ```text
/// if (*(float *)(param_1 + 8) <= 0.0)  FUN_140679510();  else  FUN_1406794b0();
/// ```
///
/// Both poll `FUN_140e6e430` and both write `saveState = 0` on a non-1 answer; only the second also
/// clears `GameMan+0xbb8` and increments `GameMan+0xbc0`. The witness hooks the first, so on any
/// frame the float is positive the save-side write happened where nothing was looking.
/// 1.17 `0x14067a300`, identical-whole over 18 instructions, both-entries.
const SAVE_STATE_SAVE_LANE_ALT_RVA: usize = 0x6794b0;

/// `FUN_14067ac90` (1.16.2) -- `GameMan::SetSaveState(int)`, the watchdog's setter. See the header:
/// the only register store to the field, one caller, and the caller is a 210-second deadline.
/// 1.17 `0x14067bae0`, identical-leaf over 3 instructions (neither image declares it in `.pdata`;
/// both decoded to the same 0xe bytes), neither-entry.
const SAVE_STATE_SETTER_RVA: usize = 0x67ac90;

/// The MenuJob load-save-data wait step `FUN_140678e00` was the writer.
pub const SAVE_STATE_SITE_MENUJOB_LOADWAIT: usize = 3;
/// The alternate save-wait wrapper `FUN_1406794b0` was the writer.
pub const SAVE_STATE_SITE_SAVE_LANE_ALT: usize = 4;
/// `GameMan::SetSaveState` was called directly -- on the only caller in the image, the 210-second
/// `MoveMapStep` watchdog.
pub const SAVE_STATE_SITE_SETTER: usize = 5;

/// Calls forwarded through `FUN_140678e00`.
static SAVE_STATE_MENUJOB_LOADWAIT_CALLS: AtomicU64 = AtomicU64::new(0);
/// Calls forwarded through `FUN_1406794b0`.
static SAVE_STATE_SAVE_LANE_ALT_CALLS: AtomicU64 = AtomicU64::new(0);
/// Calls forwarded through `GameMan::SetSaveState`.
static SAVE_STATE_SETTER_CALLS: AtomicU64 = AtomicU64::new(0);
/// The value the first `SetSaveState` call was asked to write ([`SAVE_STATE_ARG_UNOBSERVED`] until
/// one happens). The watchdog always passes 0, so anything else says the setter has a caller the
/// image scan did not find.
static SAVE_STATE_SETTER_FIRST_ARG: AtomicU32 = AtomicU32::new(SAVE_STATE_ARG_UNOBSERVED);
/// Host-epoch milliseconds of that first `SetSaveState` call, or [`ELAPSED_MS_UNAVAILABLE`].
static SAVE_STATE_SETTER_FIRST_MS: AtomicU64 = AtomicU64::new(ELAPSED_MS_UNAVAILABLE);

/// No `SetSaveState` argument has been observed. `u32::MAX` is not a state the field ever holds, so
/// it cannot be mistaken for a measured 0 -- the distinction the whole instrument turns on.
pub const SAVE_STATE_ARG_UNOBSERVED: u32 = u32::MAX;

/// Witnessed writer sites that actually installed. Zero means none of them did, which is a different
/// verdict from every counter reading zero because nothing happened.
static SAVE_STATE_WRITER_SITES_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Calls forwarded through any witnessed writer site, all five wrappers and the setter.
static SAVE_STATE_WRITER_CALLS: AtomicU64 = AtomicU64::new(0);
/// Witnessed calls that observed `saveState` leave `SAVE_OWNS` across themselves.
///
/// Deliberately blind to the device, unlike [`poll_abandoned_a_save`]. A healthy completion also
/// leaves `SAVE_OWNS` and is counted here, which is what makes a zero meaningful: it says no
/// witnessed writer moved the field off 1 at all, so a wedge that exists anyway was not written by
/// one of them.
static SAVE_STATE_WRITER_STATE_EXITS: AtomicU64 = AtomicU64::new(0);
/// [`SAVE_STATE_WRITER_CALLS`] as it stood when the wedge was first seen from the dispatch
/// ([`u64::MAX`] = the wedge was never seen).
static SAVE_STATE_WRITER_CALLS_AT_WEDGE: AtomicU64 = AtomicU64::new(u64::MAX);
/// [`SAVE_STATE_WRITER_STATE_EXITS`] at that same moment ([`u64::MAX`] = never seen).
static SAVE_STATE_WRITER_EXITS_AT_WEDGE: AtomicU64 = AtomicU64::new(u64::MAX);

/// Nothing was ever recorded here.
pub const WEDGE_SNAPSHOT_UNRECORDED: u64 = u64::MAX;

/// Record that one witnessed writer site ran, and whether `saveState` left `SAVE_OWNS` across it.
///
/// Called from every witnessed site, including the two in `save_state_witness.rs`, so the counters
/// describe the whole set rather than the half this file adds.
fn note_writer_call(before_state: Option<u32>, after_state: Option<u32>) {
    SAVE_STATE_WRITER_CALLS.fetch_add(1, Ordering::Relaxed);
    if before_state == Some(GAME_MAN_SAVE_STATE_SAVE_OWNS) && after_state != before_state {
        SAVE_STATE_WRITER_STATE_EXITS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Freeze the writer counters at the first dispatch sample that shows the wedge.
///
/// Called from [`note_wedged_dispatch`], on the first occurrence only, so the pair describes the
/// birth rather than the plateau -- the same reason every other field in that block is first-wins.
fn snapshot_writers_at_wedge() {
    SAVE_STATE_WRITER_CALLS_AT_WEDGE.store(
        SAVE_STATE_WRITER_CALLS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    SAVE_STATE_WRITER_EXITS_AT_WEDGE.store(
        SAVE_STATE_WRITER_STATE_EXITS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
}

/// Did the wedge's `saveState` write come from outside every witnessed site?
///
/// `None` when the question cannot be asked: no site installed (so silence proves nothing), or no
/// wedge was seen (so there is nothing to attribute). `Some(true)` is the decisive negative -- the
/// sites were installed, a wedge was born, and not one witnessed call had moved `saveState` off
/// `SAVE_OWNS` by then, so the write is not any of the six stores this crate watches.
///
/// Pure and total: it is the rule that turns two counters into a verdict, and a rule that can only
/// be exercised with a game attached is a rule nobody has checked.
pub fn wedge_writer_is_outside_the_witnessed_set(
    sites_installed: usize,
    exits_at_wedge: u64,
) -> Option<bool> {
    if sites_installed == 0 || exits_at_wedge == WEDGE_SNAPSHOT_UNRECORDED {
        return None;
    }
    Some(exits_at_wedge == 0)
}

/// Calls forwarded through `FUN_140678e00`. Zero with sites installed means the MenuJob load-save-data
/// wait step never ran; zero with none installed means nothing was watching.
pub fn save_state_menujob_loadwait_calls() -> u64 {
    SAVE_STATE_MENUJOB_LOADWAIT_CALLS.load(Ordering::SeqCst)
}

/// Calls forwarded through `FUN_1406794b0`, the alternate save-wait wrapper.
pub fn save_state_save_lane_alt_calls() -> u64 {
    SAVE_STATE_SAVE_LANE_ALT_CALLS.load(Ordering::SeqCst)
}

/// Calls forwarded through `GameMan::SetSaveState`. **Non-zero means the 210-second watchdog fired**,
/// because it is the only caller in the image.
pub fn save_state_setter_calls() -> u64 {
    SAVE_STATE_SETTER_CALLS.load(Ordering::SeqCst)
}

/// The value the first `SetSaveState` call wrote ([`SAVE_STATE_ARG_UNOBSERVED`] = never called).
/// The watchdog passes 0; any other value means a caller the image scan did not find.
pub fn save_state_setter_first_arg() -> u32 {
    SAVE_STATE_SETTER_FIRST_ARG.load(Ordering::SeqCst)
}

/// Host-epoch milliseconds of that first `SetSaveState` call ([`ELAPSED_MS_UNAVAILABLE`] = never
/// called, or no clock sink wired -- [`save_state_setter_calls`] tells those apart).
pub fn save_state_setter_first_ms() -> u64 {
    SAVE_STATE_SETTER_FIRST_MS.load(Ordering::SeqCst)
}

/// How many of the six witnessed writer sites installed. **Zero means nothing was watching**, and
/// every other counter in this block reads zero for that reason rather than for a measured one.
pub fn save_state_writer_sites_installed() -> usize {
    SAVE_STATE_WRITER_SITES_INSTALLED.load(Ordering::SeqCst)
}

/// Calls forwarded through any witnessed writer site.
pub fn save_state_writer_calls() -> u64 {
    SAVE_STATE_WRITER_CALLS.load(Ordering::SeqCst)
}

/// Witnessed calls across which `saveState` left `SAVE_OWNS`, device irrelevant.
pub fn save_state_writer_state_exits() -> u64 {
    SAVE_STATE_WRITER_STATE_EXITS.load(Ordering::SeqCst)
}

/// [`save_state_writer_calls`] frozen at the wedge's birth ([`WEDGE_SNAPSHOT_UNRECORDED`] = no wedge).
pub fn save_state_writer_calls_at_wedge() -> u64 {
    SAVE_STATE_WRITER_CALLS_AT_WEDGE.load(Ordering::SeqCst)
}

/// [`save_state_writer_state_exits`] frozen at the wedge's birth ([`WEDGE_SNAPSHOT_UNRECORDED`] = no
/// wedge). **Zero, with sites installed, is the decisive negative** -- see
/// [`wedge_writer_is_outside_the_witnessed_set`].
pub fn save_state_writer_exits_at_wedge() -> u64 {
    SAVE_STATE_WRITER_EXITS_AT_WEDGE.load(Ordering::SeqCst)
}

/// The one-sentence reading of the pair above, so a probe and a log line cannot disagree about it.
pub fn wedge_writer_verdict() -> &'static str {
    match wedge_writer_is_outside_the_witnessed_set(
        save_state_writer_sites_installed(),
        save_state_writer_exits_at_wedge(),
    ) {
        None if save_state_writer_sites_installed() == 0 =>
            "no writer site installed -- every counter here is silent for that reason, not because \
             nothing happened",
        None => "no wedge was seen from the dispatch this run, so there is nothing to attribute",
        Some(true) =>
            "the wedge formed with ZERO witnessed writes taking saveState off 1 -- the write did not \
             come through any store this crate watches, and the model is what to doubt next",
        Some(false) =>
            "a witnessed writer did take saveState off 1 before the wedge; read \
             oracle_save_state_first_site and _first_caller_rva for which",
    }
}

#[cfg(windows)]
static ORIG_SAVE_STATE_MENUJOB_LOADWAIT: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SAVE_STATE_SAVE_LANE_ALT: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SAVE_STATE_SETTER: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
unsafe extern "system" fn save_state_menujob_loadwait_witness(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    unsafe {
        witness_call(
            &ORIG_SAVE_STATE_MENUJOB_LOADWAIT,
            SAVE_STATE_SITE_MENUJOB_LOADWAIT,
            &SAVE_STATE_MENUJOB_LOADWAIT_CALLS,
            a,
            b,
            c,
            d,
        )
    }
}

#[cfg(windows)]
unsafe extern "system" fn save_state_save_lane_alt_witness(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    unsafe {
        witness_call(
            &ORIG_SAVE_STATE_SAVE_LANE_ALT,
            SAVE_STATE_SITE_SAVE_LANE_ALT,
            &SAVE_STATE_SAVE_LANE_ALT_CALLS,
            a,
            b,
            c,
            d,
        )
    }
}

/// Forward `GameMan::SetSaveState(int)`, recording the value it was asked to write.
///
/// Not `witness_call`: that one exists for a poll wrapper, whose answer is its return value and
/// whose write is a consequence. This is the write itself, so the argument in `ECX` is the finding
/// and there is no answer to report. Everything else is the same shape -- forward unchanged, sample
/// either side, never write the field.
///
/// # Safety
/// Installed only on the audited `SAVE_STATE_SETTER_RVA`, whose single parameter is an integer.
#[cfg(windows)]
unsafe extern "system" fn save_state_setter_witness(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let calls = SAVE_STATE_SETTER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let raw = ORIG_SAVE_STATE_SETTER.load(Ordering::SeqCst);
    if raw == 0 {
        // Unreachable: the union publishes `orig` before enabling the detour. Swallowing the write
        // would be a behaviour change, so there is nothing safe to invent -- say so and return.
        log_message(format_args!(
            "suppress: BUG -- SetSaveState witness ran with no trampoline; the game's write to \
             GameMan.saveState was DROPPED (call #{calls}, value {a})"
        ));
        return 0;
    }
    let before_state = read_save_state();
    let original: UnionFn = unsafe { core::mem::transmute::<usize, UnionFn>(raw) };
    let answer = unsafe { original(a, b, c, d) };
    let after_state = read_save_state();
    note_writer_call(before_state, after_state);
    let requested = (a & 0xffff_ffff) as u32;
    let stamp = elapsed_ms();
    if SAVE_STATE_SETTER_FIRST_ARG
        .compare_exchange(
            SAVE_STATE_ARG_UNOBSERVED,
            requested,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        SAVE_STATE_SETTER_FIRST_MS.store(stamp, Ordering::SeqCst);
        // Always on the first call. The only caller in the image is a 210-second deadline that
        // releases nothing, so its first firing is the most consequential line this crate can emit.
        log_message(format_args!(
            "suppress: GameMan::SetSaveState({requested}) CALLED (#{calls}) -- the only caller in \
             the image is the MoveMapStep 210-second watchdog at FUN_140aff640, which forces \
             saveState without touching the SL device. saveState {} -> {}. Stamp: {}. Device: {}",
            match before_state {
                Some(state) => state.to_string(),
                None => "unreadable".to_owned(),
            },
            match after_state {
                Some(state) => state.to_string(),
                None => "unreadable".to_owned(),
            },
            match stamp {
                ELAPSED_MS_UNAVAILABLE => "unstamped (no clock sink wired)".to_owned(),
                ms => format!("+{ms}ms"),
            },
            describe_slot(read_sl_slot())
        ));
        publish_snapshot();
    }
    // The device read is the expensive half and the predicate cannot be true unless `saveState`
    // left SAVE_OWNS, so it is taken only then -- the same shape, and the same reason, as
    // `witness_call`.
    let after = (before_state == Some(GAME_MAN_SAVE_STATE_SAVE_OWNS) && after_state != before_state)
        .then(read_sl_slot)
        .flatten();
    if poll_abandoned_a_save(before_state, after_state, after) {
        note_abandoning_write(SAVE_STATE_SITE_SETTER, after_state, after, answer);
    }
    answer
}

/// Chain the three remaining writer observers. Never fatal: a failure costs attribution, not saving.
///
/// Registered through [`register_union_hook`] exactly like `install_save_state_witness`, so the
/// product's existing detours on these addresses (there are none today) would chain rather than
/// compete, and so the single 1.16.2 -> 1.17 resolve stays inside the union.
#[cfg(windows)]
pub fn install_save_state_writers() {
    for (rva, handler, orig, name) in [
        (
            SAVE_STATE_MENUJOB_LOADWAIT_RVA,
            save_state_menujob_loadwait_witness as UnionFn,
            &ORIG_SAVE_STATE_MENUJOB_LOADWAIT,
            "menujob load-save-data wait 0x678e00",
        ),
        (
            SAVE_STATE_SAVE_LANE_ALT_RVA,
            save_state_save_lane_alt_witness as UnionFn,
            &ORIG_SAVE_STATE_SAVE_LANE_ALT,
            "alternate save-wait 0x6794b0",
        ),
        (
            SAVE_STATE_SETTER_RVA,
            save_state_setter_witness as UnionFn,
            &ORIG_SAVE_STATE_SETTER,
            "GameMan::SetSaveState 0x67ac90",
        ),
    ] {
        // Non-resolving on purpose (`game_rva_for_hook`), like `install_save_state_witness`:
        // `register_union_hook` owns the single resolve, and resolving twice can land the detour on
        // a third function in silence.
        let Ok(address) = er_game_base::mem::game_rva_for_hook(rva as u32) else {
            log_message(format_args!(
                "suppress: save-state writer witness could not resolve {name}; a saveState write \
                 from that site will stay unattributed this run"
            ));
            continue;
        };
        match unsafe { register_union_hook(address, handler, orig) } {
            Ok(()) => {
                SAVE_STATE_WRITER_SITES_INSTALLED.fetch_add(1, Ordering::SeqCst);
                log_message(format_args!(
                    "suppress: save-state writer witness chained on {name} (0x{address:x}) -- \
                     observer only, forwards every call unchanged"
                ));
            }
            Err(status) => log_message(format_args!(
                "suppress: save-state writer witness failed on {name}: {status:?}; a saveState \
                 write from that site will stay unattributed this run"
            )),
        }
    }
}

// The rules this file adds are decisions about what a pair of counters means, not readings of a
// runtime value, so they are exercised with no game attached -- the same reason
// `poll_abandoned_a_save` and `dispatch_sample_is_wedged` are.
#[cfg(test)]
mod save_state_writers_tests {
    use super::*;

    /// The decisive negative, and the two ways of refusing to draw it.
    #[test]
    fn a_wedge_with_no_witnessed_state_exit_indicts_the_model_not_a_site() {
        // The FINDING: sites installed, a wedge recorded, and not one witnessed call moved
        // `saveState` off 1 before it.
        assert_eq!(
            wedge_writer_is_outside_the_witnessed_set(6, 0),
            Some(true)
        );
        // A witnessed writer did move it: the attribution fields say which, and this verdict must
        // not claim otherwise.
        assert_eq!(
            wedge_writer_is_outside_the_witnessed_set(6, 3),
            Some(false)
        );
        // Nothing installed: every counter is zero for that reason. Reporting `true` here would be
        // the exact "zero means proved" confusion the sentinels exist to prevent.
        assert_eq!(wedge_writer_is_outside_the_witnessed_set(0, 0), None);
        // No wedge seen: there is nothing to attribute, and a run that simply behaved must not read
        // as a finding.
        assert_eq!(
            wedge_writer_is_outside_the_witnessed_set(6, WEDGE_SNAPSHOT_UNRECORDED),
            None
        );
        // ...not even when some sites installed and some did not.
        assert_eq!(
            wedge_writer_is_outside_the_witnessed_set(2, WEDGE_SNAPSHOT_UNRECORDED),
            None
        );
    }

    /// Every one of the counters this file adds must read as absent before anything runs, never as a
    /// measured zero -- the property `_load_poll_calls`/`_save_lane_calls` carry for the original
    /// witness, restated for the fields that have no counter of their own.
    #[test]
    fn an_unobserved_writer_set_reads_as_absent_not_as_zero() {
        assert_eq!(save_state_writer_sites_installed(), 0);
        assert_eq!(save_state_setter_first_arg(), SAVE_STATE_ARG_UNOBSERVED);
        assert_eq!(save_state_setter_first_ms(), ELAPSED_MS_UNAVAILABLE);
        assert_eq!(save_state_writer_calls_at_wedge(), WEDGE_SNAPSHOT_UNRECORDED);
        assert_eq!(save_state_writer_exits_at_wedge(), WEDGE_SNAPSHOT_UNRECORDED);
        // And the sentinel is not a value the field can hold: `saveState` is a small enum, so
        // `u32::MAX` can never be mistaken for a state the game actually wrote.
        assert_ne!(SAVE_STATE_ARG_UNOBSERVED, GAME_MAN_SAVE_STATE_IDLE);
        assert_ne!(SAVE_STATE_ARG_UNOBSERVED, GAME_MAN_SAVE_STATE_SAVE_OWNS);
        assert_ne!(SAVE_STATE_ARG_UNOBSERVED, GAME_MAN_SAVE_STATE_LOAD_OWNS);
        assert_ne!(SAVE_STATE_ARG_UNOBSERVED, GAME_MAN_SAVE_STATE_LOAD_RESIDENT);
        // The verdict must say it is silent for lack of an instrument, not report a clean run.
        assert!(wedge_writer_verdict().contains("no writer site installed"));
    }

    /// `note_writer_call` counts an exit from `SAVE_OWNS` and nothing else, because a count that
    /// also fired on healthy polling could never make a zero mean anything.
    #[test]
    fn only_leaving_save_owns_counts_as_a_writer_state_exit() {
        let before = save_state_writer_state_exits();
        // Still in flight: the answer on almost every frame of a real save.
        note_writer_call(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
        );
        // The save side was not even involved.
        note_writer_call(
            Some(GAME_MAN_SAVE_STATE_LOAD_OWNS),
            Some(GAME_MAN_SAVE_STATE_IDLE),
        );
        // Unknown on either side is never evidence.
        note_writer_call(None, Some(GAME_MAN_SAVE_STATE_IDLE));
        note_writer_call(Some(GAME_MAN_SAVE_STATE_SAVE_OWNS), None);
        assert_eq!(save_state_writer_state_exits(), before + 1);
        // ...and that last one was an exit: 1 -> unreadable is still a departure from SAVE_OWNS,
        // and pretending otherwise would let an unreadable sample hide the write.
        note_writer_call(
            Some(GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(GAME_MAN_SAVE_STATE_IDLE),
        );
        assert_eq!(save_state_writer_state_exits(), before + 2);
        assert!(save_state_writer_calls() >= 5);
    }

    /// The three new sites must be distinguishable from each other and from the two that existed,
    /// or the one thing the instrument produces -- a name -- is wasted.
    #[test]
    fn every_writer_site_has_its_own_label() {
        let labels: Vec<&str> = [
            SAVE_STATE_SITE_NONE,
            SAVE_STATE_SITE_LOAD_POLL,
            SAVE_STATE_SITE_SAVE_LANE,
            SAVE_STATE_SITE_MENUJOB_LOADWAIT,
            SAVE_STATE_SITE_SAVE_LANE_ALT,
            SAVE_STATE_SITE_SETTER,
        ]
        .into_iter()
        .map(save_state_site_label)
        .collect();
        for (i, label) in labels.iter().enumerate() {
            // Index 0 is the unknown site and is named "none" on purpose; every other index must
            // have earned a name of its own rather than falling through to it.
            if i > 0 {
                assert_ne!(*label, "none", "site {i} fell through to the unknown label");
            }
            assert!(!labels[..i].contains(label), "duplicate label {label}");
        }
    }

    /// None of the three new sites may write the field. Only the load poll repairs, and that
    /// decision is about which native branch is being undone -- adding sites must not widen it.
    #[test]
    fn no_new_writer_site_touches_game_memory() {
        assert!(!save_state_site_takes_the_repair(
            SAVE_STATE_SITE_MENUJOB_LOADWAIT
        ));
        assert!(!save_state_site_takes_the_repair(
            SAVE_STATE_SITE_SAVE_LANE_ALT
        ));
        assert!(!save_state_site_takes_the_repair(SAVE_STATE_SITE_SETTER));
    }
}
