// Which submit LATCHED the device, and did its lane accept.
//
// `include!`d into `lib.rs` like the blocks around it, and built on the same observer shape as
// `save_state_witness.rs` / `save_state_writers.rs`: forward every call unchanged, sample either
// side, write no game memory. It adds no fifth mechanism -- it moves the vantage point.
//
// Why the vantage point had to move. Four rounds of instruments watched `GameMan.saveState` and
// asked which store took it off 1. The answer, measured on 2026-08-31 run `wedge-writers-20260831-d`
// with all six stores in the image witnessed, was NONE: `writer_state_exits = 2`, both of them
// healthy completions with the device clear, and `exits_at_wedge = 0`. A wedge existed anyway --
// `iodev+0x10 = 0x9c9a1580`, `iodev+0x20 = 0xacc43e40`, `saveState = 0`.
//
// There is exactly one shape left that produces that reading with no zero-writer at all: a submit
// that LATCHED the device and whose lane never accepted, so `saveState` was never written to 1 in
// the first place and never had to be written back down. The writer set being complete and silent
// is not a contradiction under that shape -- it is what that shape predicts.
//
// So the question this file answers is the one neither the writer witness nor the reload trace
// carries: for the `SLSaveContent` sitting on the device at the wedge, which submit put it there,
// what did that submit's builder return, and did the lane that called it go on to accept?
//
// The three sites, and why these three (1.16.2 decompiles, shift 0; the 1.17 bodies are byte-equal
// under the alignment key -- see the derivation block below):
//
//   `FUN_140e6ef60`  the combined lane's submit builder. Sole caller `FUN_14067b940`, the lane the
//                    2026-08-31 wedge sample names (`lane = 3`). Writes `iodev+0x10` before its
//                    heap-capability guard and can return 0 afterwards.
//   `FUN_140e6ec70`  the CHARACTER/SYSTEM builder -- a 15-byte tail-call dispatcher, `cmp
//                    byte [rcx+0x40],0 / jne FUN_140e6f760 / jmp FUN_140e6f940`, so hooking it sees
//                    both sub-builders and all three of its lanes (`FUN_14067b750`,
//                    `FUN_14067b570`, `FUN_14067bc10`) in one place.
//   `FUN_140e6fb50`  the ENQUEUE, and the reason a negative here still means something: it is the
//                    only writer of `iodev+0x20` and every builder in the subsystem funnels through
//                    it, including `FUN_140e6ec80` (the preview lane's builder, which is not
//                    hooked). A device holding a non-zero `+0x20` that this site never saw was
//                    latched by something outside the whole SL submit path.
//
// What the static read already settles, so a run is not asked to re-derive it. Every builder's
// failure path was decompiled rather than assumed:
//
//   * `FUN_140e6fb50` stores its job into `iodev+0x20` and, when `FUN_14240ae10` hands back null,
//     calls `FUN_140e6f200` -- the full release -- before returning false. An enqueue that fails
//     therefore leaves the device clean, and "the enqueue failed and stranded the content" is
//     false for every lane.
//   * `FUN_140e6ec70`'s `param_3 >= 0xc` fall-out calls `FUN_140e6f200` too. bd
//     `e6ec70-can-latch-the-device-without-accepting-so-savestate-is-never-written-2026-08-31`
//     records that arm as a latch-without-accept hole with no release; the 1.16.2 decompile read
//     for this file has the release, so that memory's rendering is wrong about this build. It is
//     recorded here rather than silently dropped, because the two readings disagree and the code
//     is the one that runs.
//   * `FUN_140e6f940`'s deferred arm (`iodev+0x28 != 0`) returns 1 with `+0x20` still null -- an
//     accept with no job. That is a real latch shape, and it is the one the 2026-08-31 sample
//     excludes on its own evidence (`+0x28 == 0`, `+0x20` a live pointer).
//
// None of that names the frame. A builder that returns 0 with `+0x10` still populated is a fact
// about one call, and the only way to have it is to be standing there when it happens.
//
// Address derivation, and why it is not a constant. `0x140e6ef60` and `0x140e6ec70` are not rows in
// `rva-map-1162-to-1170.verified.tsv` or `...needed-verified.tsv`, so `resolve_detour_address`
// would refuse them and the hooks would silently not exist. Rather than widen a ledger this branch
// does not own, each builder is derived from the running image, which is what
// `MhHook::new_runtime_derived` / `register_union_hook_runtime_derived` exist for:
//
//   1. its calling lane is in the ledger and is `IDENTICAL-WHOLE` with identical `.pdata` extents
//      (`0x67b940 -> 0x67c790` PDATA:0x2cc/0x2cc; `0x67b750 -> 0x67c5a0` 0x1e3/0x1e3;
//      `0x67b570 -> 0x67c3c0` 0x1d6/0x1d6; `0x67bc10 -> 0x67ca60` 0x11f/0x11f), so the offset of a
//      call inside it survives the move;
//   2. `resolve_call_site_rva` places that call on the running build;
//   3. the `E8 rel32` there is DECODED, so the address comes out of the image rather than out of
//      arithmetic on a base;
//   4. `write_site_is_sound` then asks the running image's own `.pdata` whether the destination is
//      a function entry (or an unwind-less leaf) with room for MinHook's five bytes.
//
// The char builder is derived from all three of its lanes and the three answers must agree, which
// is a check no single derivation can make on itself.
//
// Offline evidence for the pairs, recorded here because it cannot go in a ledger this branch may
// not write. `scripts/diff-function-bodies-1162-1170.py`, `.pdata` extents from
// `scripts/pdata-lookup-1162-1170.py`, and the `E8` decode from both images:
//
//   1.16.2      1.17        verdict                          entry evidence        patch site
//   0xe6ef60    0xe70d60    identical-whole 1.000/134 insns  both-entries 0x210    7B relocatable
//   0xe6ec70    0xe70a70    identical-leaf  1.000/3 insns    neither-entry 0xf     10B relocatable
//   0xe6fb50    0xe71950    identical-whole 1.000/59 insns   both-entries 0xe2     8B relocatable
//
// Both new verdicts are in `EXHAUSTIVE_VERDICTS`, so neither is a loosened score; they are recorded
// as prose here only because the row cannot be added to `docs/recon/rva-map-*.tsv` from this branch.
// `0xe6fb50` is already an audited row and resolves through the table like any other constant.
//
// The preview lane is deliberately not hooked. `FUN_140e6ec80` (`0xe70a80` on 1.17) is the fourth
// builder, called only by `FUN_14067b4e0`. Its 1.17 entry sits exactly 0x10 bytes past the char
// stub's, which is the overlap window `scripts/audit-1170-hook-targets.py` refuses -- two detours
// that close together share MinHook's patch/relocation neighbourhood. The enqueue observer covers
// its `+0x20` write instead, and a latch that reaches no site at all is reported as
// [`LATCH_MATCH_NO_OBSERVATION`] rather than as an absence.

/// `FUN_14067b940` (1.16.2), the combined save dispatch lane; `0x67c790` on 1.17.
const SUBMIT_LANE_COMBINED_RVA: usize = 0x67b940;
/// Offset of `call FUN_140e6ef60` inside it (`0x14067bb2e - 0x14067b940`). Decodes to `0x140e6ef60`
/// in 1.16.2 and `0x140e70d60` in 1.17 at the same offset, checked against both image files.
const SUBMIT_LANE_COMBINED_CALL_OFFSET: usize = 0x1ee;

/// `FUN_14067b750` (1.16.2), the character-slot lane; `0x67c5a0` on 1.17.
const SUBMIT_LANE_CHAR_RVA: usize = 0x67b750;
/// Offset of its `call FUN_140e6ec70` (`0x14067b877 - 0x14067b750`).
const SUBMIT_LANE_CHAR_CALL_OFFSET: usize = 0x127;

/// `FUN_14067b570` (1.16.2), the system-slot lane; `0x67c3c0` on 1.17.
const SUBMIT_LANE_SYSTEM_RVA: usize = 0x67b570;
/// Offset of its `call FUN_140e6ec70` (`0x14067b6c4 - 0x14067b570`).
const SUBMIT_LANE_SYSTEM_CALL_OFFSET: usize = 0x154;

/// `FUN_14067bc10` (1.16.2), the third caller of the char builder; `0x67ca60` on 1.17.
const SUBMIT_LANE_ENTRY0B_RVA: usize = 0x67bc10;
/// Offset of its `call FUN_140e6ec70` (`0x14067bc94 - 0x14067bc10`).
const SUBMIT_LANE_ENTRY0B_CALL_OFFSET: usize = 0x84;

/// `E8` -- a near `call rel32`, the only encoding any of the four call sites uses. Decoding refuses
/// anything else rather than treating four arbitrary bytes as a displacement.
const CALL_REL32_OPCODE: u8 = 0xe8;
/// Length of `E8 rel32`, which is also the distance from the site to the next instruction.
const CALL_REL32_LEN: usize = 5;

/// No submit site has been observed latching anything.
pub const SUBMIT_SITE_NONE: usize = 0;
/// `FUN_140e6ef60`, the combined lane's builder.
pub const SUBMIT_SITE_COMBINED_BUILDER: usize = 1;
/// `FUN_140e6ec70`, the CHARACTER/SYSTEM builder (both tail-call arms).
pub const SUBMIT_SITE_CHAR_BUILDER: usize = 2;
/// `FUN_140e6fb50`, the enqueue -- the sole writer of `iodev+0x20`.
pub const SUBMIT_SITE_ENQUEUE: usize = 3;

/// Name the site that latched a request.
pub fn submit_site_label(site: usize) -> &'static str {
    match site {
        SUBMIT_SITE_COMBINED_BUILDER => "combined-builder-0xe6ef60",
        SUBMIT_SITE_CHAR_BUILDER => "char-builder-0xe6ec70",
        SUBMIT_SITE_ENQUEUE => "enqueue-0xe6fb50",
        _ => "none",
    }
}

/// No return value has been observed. `u64::MAX` is not a value any of these functions can return
/// (all three return a `bool`/`char` in `AL`), so it cannot be mistaken for a measured 0 -- the
/// distinction this whole instrument turns on.
pub const SUBMIT_RETURN_UNOBSERVED: u64 = u64::MAX;
/// No `saveState` sample was taken. `u32::MAX` is not a state the field holds.
pub const SUBMIT_STATE_UNOBSERVED: u32 = u32::MAX;

/// The wedge has not been reached, so there is nothing to attribute.
pub const LATCH_MATCH_UNRECORDED: usize = 0;
/// A wedge was born and no latch had ever been observed at any installed site. With sites
/// installed that is a finding in its own right: the content on the device was put there by
/// something outside the SL submit path this file watches.
pub const LATCH_MATCH_NO_OBSERVATION: usize = 1;
/// The content on the device at the wedge is the one the last observed latch installed. The
/// builder/lane fields beside it describe that submit.
pub const LATCH_MATCH_SAME: usize = 2;
/// A latch was observed, but for a different content pointer than the one wedged -- so the wedged
/// content was latched before the observers installed, or by an unwatched path, and the recorded
/// builder/lane fields describe a different submit and must not be read as its attribution.
pub const LATCH_MATCH_DIFFERENT: usize = 3;

/// Name a match code, so a probe and a log line cannot disagree about which of the four it is.
pub fn latch_match_label(code: usize) -> &'static str {
    match code {
        LATCH_MATCH_NO_OBSERVATION => "no-latch-ever-observed",
        LATCH_MATCH_SAME => "same-content-attributed",
        LATCH_MATCH_DIFFERENT => "different-content-unattributed",
        _ => "no-wedge-recorded",
    }
}

/// How many of the three submit sites chained. **Zero means nothing was watching**, and every other
/// counter in this block reads zero for that reason rather than for a measured one. Read it first.
static SUBMIT_SITES_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Calls forwarded through `FUN_140e6ef60`.
static SUBMIT_COMBINED_CALLS: AtomicU64 = AtomicU64::new(0);
/// Calls forwarded through `FUN_140e6ec70`.
static SUBMIT_CHAR_CALLS: AtomicU64 = AtomicU64::new(0);
/// Calls forwarded through `FUN_140e6fb50`.
static SUBMIT_ENQUEUE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Observed transitions of `iodev+0x10` from 0 to a pointer -- the latch itself.
static SUBMIT_LATCHES: AtomicU64 = AtomicU64::new(0);
/// Latches whose builder then returned 0 with the content still on the device. **This is the shape
/// the whole file exists to count**: a submit that owns the device and whose lane will therefore
/// never write `saveState = 1`.
static SUBMIT_LATCHES_WITHOUT_ACCEPT: AtomicU64 = AtomicU64::new(0);
/// Builder calls that latched and then released before returning (`FUN_140e6f200` on a failure
/// path). Counted so a zero in [`SUBMIT_LATCHES_WITHOUT_ACCEPT`] can be read as "no builder
/// stranded one" rather than "no builder ever failed".
static SUBMIT_LATCHES_SELF_RELEASED: AtomicU64 = AtomicU64::new(0);

/// The content pointer the most recent observed latch installed (`iodev+0x10`).
static LATCH_CONTENT: AtomicUsize = AtomicUsize::new(0);
/// `iodev+0x20` as it stood when that latch's site returned. Zero means the submit was accepted
/// without a job (the deferred `iodev+0x28 != 0` arm) or the enqueue had not run yet.
static LATCH_JOB: AtomicUsize = AtomicUsize::new(0);
/// [`SUBMIT_SITE_*`](SUBMIT_SITE_COMBINED_BUILDER) of that latch.
static LATCH_SITE: AtomicUsize = AtomicUsize::new(SUBMIT_SITE_NONE);
/// The site's own return value at that latch ([`SUBMIT_RETURN_UNOBSERVED`] = none seen).
static LATCH_BUILDER_RETURN: AtomicU64 = AtomicU64::new(SUBMIT_RETURN_UNOBSERVED);
/// `GameMan.saveState` immediately after the site returned. The lane writes 1 after the builder
/// returns, so a healthy accept reads 0 here and 1 in [`LATCH_LANE_STATE`].
static LATCH_BUILDER_STATE: AtomicU32 = AtomicU32::new(SUBMIT_STATE_UNOBSERVED);
/// Host-epoch milliseconds of that latch ([`ELAPSED_MS_UNAVAILABLE`] = unstamped).
static LATCH_MS: AtomicU64 = AtomicU64::new(ELAPSED_MS_UNAVAILABLE);
/// `SAVE_LANE_*` of the dispatch lane that closed that latch ([`SAVE_LANE_NONE`] = the lane was
/// not one of the three `observe_dispatch` wraps, so the close never happened).
static LATCH_LANE: AtomicUsize = AtomicUsize::new(SAVE_LANE_NONE);
/// The lane's return value ([`SUBMIT_RETURN_UNOBSERVED`] = the lane never closed this latch).
static LATCH_LANE_RETURN: AtomicU64 = AtomicU64::new(SUBMIT_RETURN_UNOBSERVED);
/// `GameMan.saveState` sampled after the lane returned. **This is the accept**: the lane's commit
/// tail writes 1 there, so `1` means it accepted and anything else means it did not.
static LATCH_LANE_STATE: AtomicU32 = AtomicU32::new(SUBMIT_STATE_UNOBSERVED);

/// [`LATCH_MATCH_*`](LATCH_MATCH_SAME) frozen at the wedge's birth.
static WEDGE_LATCH_MATCH: AtomicUsize = AtomicUsize::new(LATCH_MATCH_UNRECORDED);
/// [`LATCH_SITE`] frozen at the wedge's birth.
static WEDGE_LATCH_SITE: AtomicUsize = AtomicUsize::new(SUBMIT_SITE_NONE);
/// [`LATCH_BUILDER_RETURN`] frozen at the wedge's birth.
static WEDGE_LATCH_BUILDER_RETURN: AtomicU64 = AtomicU64::new(SUBMIT_RETURN_UNOBSERVED);
/// [`LATCH_LANE_RETURN`] frozen at the wedge's birth.
static WEDGE_LATCH_LANE_RETURN: AtomicU64 = AtomicU64::new(SUBMIT_RETURN_UNOBSERVED);
/// [`LATCH_LANE_STATE`] frozen at the wedge's birth.
static WEDGE_LATCH_LANE_STATE: AtomicU32 = AtomicU32::new(SUBMIT_STATE_UNOBSERVED);
/// [`LATCH_CONTENT`] frozen at the wedge's birth, so a reader can compare it against the wedge's
/// own `+0x10` without trusting that nothing latched in between.
static WEDGE_LATCH_CONTENT: AtomicUsize = AtomicUsize::new(0);
/// [`LATCH_MS`] frozen at the wedge's birth.
static WEDGE_LATCH_MS: AtomicU64 = AtomicU64::new(ELAPSED_MS_UNAVAILABLE);

/// Did the lane that called this submit accept it?
///
/// The whole question in one rule, so it can be exercised with no game attached. A lane accepts by
/// running its commit tail, which is guarded on the builder's return and ends `saveState = 1`; so
/// an accept is a non-zero lane return and `saveState` reading `SAVE_OWNS` afterwards. Either half
/// alone is not enough:
///
/// * a non-zero return with `saveState` still idle means the tail did not run (or ran and was
///   immediately undone), which is precisely the state that strands a latched device;
/// * `SAVE_OWNS` with a zero return would mean something else owns the mutex, not this submit.
///
/// `None` when the lane never closed the latch -- the two lanes `observe_dispatch` does not wrap,
/// or a submit built outside a lane. "Unknown" is never reported as either answer.
pub fn lane_accepted(lane_return: u64, lane_state: u32) -> Option<bool> {
    if lane_return == SUBMIT_RETURN_UNOBSERVED || lane_state == SUBMIT_STATE_UNOBSERVED {
        return None;
    }
    Some(lane_return & 0xff != 0 && lane_state == GAME_MAN_SAVE_STATE_SAVE_OWNS)
}

/// Classify a wedge sample against the latch record.
///
/// Pure and total: it is the rule that decides whether the recorded builder/lane fields describe
/// the wedged request or merely happen to be the last thing that ran, and getting that wrong would
/// attribute a wedge to an unrelated submit. `wedge_content` is `iodev+0x10` at the wedge;
/// `latched_content` is the last content this file observed being latched, or `None` when it never
/// observed one.
pub fn classify_wedge_latch(wedge_content: usize, latched_content: Option<usize>) -> usize {
    match latched_content {
        None => LATCH_MATCH_NO_OBSERVATION,
        Some(content) if content == wedge_content => LATCH_MATCH_SAME,
        Some(_) => LATCH_MATCH_DIFFERENT,
    }
}

/// The one-sentence reading of the whole block, so a probe and a log line cannot disagree.
///
/// Ordered by what a reader must not skip: nothing installed beats every other reading, because
/// then no field below it was measured.
pub fn wedge_latch_verdict(
    sites_installed: usize,
    match_code: usize,
    builder_return: u64,
    lane_return: u64,
    lane_state: u32,
) -> &'static str {
    if sites_installed == 0 {
        return "no submit site installed -- every counter here is silent for that reason, not \
                because nothing latched the device";
    }
    match match_code {
        LATCH_MATCH_UNRECORDED => {
            "no wedge was seen from the dispatch this run, so there is nothing to attribute"
        }
        LATCH_MATCH_NO_OBSERVATION =>
            "a wedge was born and NOT ONE latch was observed at any watched submit site -- the \
             content on the device was put there outside FUN_140e6ef60/FUN_140e6ec70/FUN_140e6fb50",
        LATCH_MATCH_DIFFERENT =>
            "a wedge was born holding content that no observed latch installed -- it predates the \
             observers or came from the unhooked PREVIEW builder FUN_140e6ec80; the builder and \
             lane fields describe a DIFFERENT submit and are not its attribution",
        _ => match (builder_return & 0xff != 0, lane_accepted(lane_return, lane_state)) {
            (true, Some(true)) =>
                "the wedged content was latched by an ACCEPTED submit: the builder returned \
                 non-zero and its lane wrote saveState = 1, so saveState DID leave 0 and something \
                 later took it back down -- back to the writer set",
            (true, Some(false)) =>
                "THE FINDING: the builder ACCEPTED (non-zero) but its lane did NOT commit -- the \
                 device is latched and saveState never reached 1, so no writer was ever needed to \
                 produce the wedge",
            (false, _) =>
                "THE FINDING: the builder RETURNED ZERO with the content still on the device, so \
                 the lane's commit tail never ran and saveState was never written to 1 -- the \
                 wedge needs no zero-writer at all",
            (true, None) =>
                "the wedged content was latched by a builder that returned non-zero, but no \
                 observed lane closed it (FUN_14067bc10 / FUN_14067b4e0 are not wrapped), so \
                 whether the commit tail ran is unmeasured this run",
        },
    }
}

/// How many of the three submit sites installed. Zero means nothing watched.
pub fn submit_sites_installed() -> usize {
    SUBMIT_SITES_INSTALLED.load(Ordering::SeqCst)
}

/// Calls forwarded through `FUN_140e6ef60`, the combined lane's builder.
pub fn submit_combined_calls() -> u64 {
    SUBMIT_COMBINED_CALLS.load(Ordering::SeqCst)
}

/// Calls forwarded through `FUN_140e6ec70`, the CHARACTER/SYSTEM builder.
pub fn submit_char_calls() -> u64 {
    SUBMIT_CHAR_CALLS.load(Ordering::SeqCst)
}

/// Calls forwarded through `FUN_140e6fb50`, the enqueue.
pub fn submit_enqueue_calls() -> u64 {
    SUBMIT_ENQUEUE_CALLS.load(Ordering::SeqCst)
}

/// Observed `iodev+0x10` transitions from 0 to a pointer.
pub fn submit_latches() -> u64 {
    SUBMIT_LATCHES.load(Ordering::SeqCst)
}

/// Latches whose site returned 0 with the content still resident. **Non-zero is the shape that
/// produces a wedge with no `saveState` writer at all.**
pub fn submit_latches_without_accept() -> u64 {
    SUBMIT_LATCHES_WITHOUT_ACCEPT.load(Ordering::SeqCst)
}

/// Latches the site itself undid before returning, via `FUN_140e6f200`. Counted so a zero in
/// [`submit_latches_without_accept`] means "none was stranded", not "none ever failed".
pub fn submit_latches_self_released() -> u64 {
    SUBMIT_LATCHES_SELF_RELEASED.load(Ordering::SeqCst)
}

/// The content pointer of the most recent observed latch, or `None` when none was observed.
pub fn latch_content() -> Option<usize> {
    (SUBMIT_LATCHES.load(Ordering::SeqCst) != 0).then(|| LATCH_CONTENT.load(Ordering::SeqCst))
}

/// `iodev+0x20` at that latch's site return.
pub fn latch_job() -> usize {
    LATCH_JOB.load(Ordering::SeqCst)
}

/// [`SUBMIT_SITE_*`](SUBMIT_SITE_COMBINED_BUILDER) of the most recent observed latch.
pub fn latch_site() -> usize {
    LATCH_SITE.load(Ordering::SeqCst)
}

/// The most recent latch's builder return ([`SUBMIT_RETURN_UNOBSERVED`] = none).
pub fn latch_builder_return() -> u64 {
    LATCH_BUILDER_RETURN.load(Ordering::SeqCst)
}

/// `saveState` immediately after that builder returned ([`SUBMIT_STATE_UNOBSERVED`] = unsampled).
pub fn latch_builder_state() -> u32 {
    LATCH_BUILDER_STATE.load(Ordering::SeqCst)
}

/// Host-epoch milliseconds of the most recent latch.
pub fn latch_ms() -> u64 {
    LATCH_MS.load(Ordering::SeqCst)
}

/// `SAVE_LANE_*` of the lane that closed the most recent latch.
pub fn latch_lane() -> usize {
    LATCH_LANE.load(Ordering::SeqCst)
}

/// The closing lane's return ([`SUBMIT_RETURN_UNOBSERVED`] = no lane closed it).
pub fn latch_lane_return() -> u64 {
    LATCH_LANE_RETURN.load(Ordering::SeqCst)
}

/// `saveState` after that lane returned ([`SUBMIT_STATE_UNOBSERVED`] = unsampled).
pub fn latch_lane_state() -> u32 {
    LATCH_LANE_STATE.load(Ordering::SeqCst)
}

/// [`LATCH_MATCH_*`](LATCH_MATCH_SAME) frozen at the wedge's birth.
pub fn wedge_latch_match() -> usize {
    WEDGE_LATCH_MATCH.load(Ordering::SeqCst)
}

/// The site that latched the wedged content ([`SUBMIT_SITE_NONE`] = unattributed).
pub fn wedge_latch_site() -> usize {
    WEDGE_LATCH_SITE.load(Ordering::SeqCst)
}

/// The builder return of the submit that latched the wedged content.
pub fn wedge_latch_builder_return() -> u64 {
    WEDGE_LATCH_BUILDER_RETURN.load(Ordering::SeqCst)
}

/// The lane return of that same submit.
pub fn wedge_latch_lane_return() -> u64 {
    WEDGE_LATCH_LANE_RETURN.load(Ordering::SeqCst)
}

/// `saveState` after that lane returned -- the accept, or its absence.
pub fn wedge_latch_lane_state() -> u32 {
    WEDGE_LATCH_LANE_STATE.load(Ordering::SeqCst)
}

/// The content pointer the record held at the wedge's birth, or `None` when no latch had ever
/// been observed by then. `None` and `Some(0)` would read the same as a number, and the difference
/// is the difference between "nothing was watching" and "something was and saw nothing".
pub fn wedge_latch_content() -> Option<usize> {
    (WEDGE_LATCH_MATCH.load(Ordering::SeqCst) != LATCH_MATCH_NO_OBSERVATION
        && WEDGE_LATCH_MATCH.load(Ordering::SeqCst) != LATCH_MATCH_UNRECORDED)
        .then(|| WEDGE_LATCH_CONTENT.load(Ordering::SeqCst))
}

/// Host-epoch milliseconds of the latch attributed to the wedge.
pub fn wedge_latch_ms() -> u64 {
    WEDGE_LATCH_MS.load(Ordering::SeqCst)
}

/// Did the lane accept the submit that latched the wedged content? `None` when unmeasured.
pub fn wedge_lane_accepted() -> Option<bool> {
    lane_accepted(wedge_latch_lane_return(), wedge_latch_lane_state())
}

/// The verdict, assembled from this block's own published values.
pub fn submit_latch_verdict() -> &'static str {
    wedge_latch_verdict(
        submit_sites_installed(),
        wedge_latch_match(),
        wedge_latch_builder_return(),
        wedge_latch_lane_return(),
        wedge_latch_lane_state(),
    )
}

// ---- the observers ---------------------------------------------------------------------------

/// Record one site call: sample the device either side and latch the transition when there is one.
///
/// Cold relative to the forwarding itself, and it takes the second device read only when the first
/// said the device was free -- so an ordinary declining frame (device already latched) costs one
/// read, the same as `observe_dispatch` already pays.
#[cfg(windows)]
fn note_submit_call(site: usize, before: Option<SlRequestSlot>, answer: usize) {
    let Some(before) = before else { return };
    if before.save_content != 0 {
        // The device was already holding something on the way in, so this call cannot be the one
        // that latched it. Nothing to attribute, and no second read to pay for.
        return;
    }
    let Some(after) = read_sl_slot() else { return };
    if after.save_content == 0 {
        // Refused at its own precondition, or latched and released again inside the call
        // (`FUN_140e6f200` on a failure path). Both leave the device as it was found.
        if answer & 0xff == 0 {
            SUBMIT_LATCHES_SELF_RELEASED.fetch_add(1, Ordering::Relaxed);
        }
        return;
    }
    let total = SUBMIT_LATCHES.fetch_add(1, Ordering::SeqCst) + 1;
    let state_after = read_save_state();
    let stamp = elapsed_ms();
    LATCH_CONTENT.store(after.save_content, Ordering::SeqCst);
    LATCH_JOB.store(after.job, Ordering::SeqCst);
    LATCH_SITE.store(site, Ordering::SeqCst);
    LATCH_BUILDER_RETURN.store(answer as u64, Ordering::SeqCst);
    LATCH_BUILDER_STATE.store(
        state_after.unwrap_or(SUBMIT_STATE_UNOBSERVED),
        Ordering::SeqCst,
    );
    LATCH_MS.store(stamp, Ordering::SeqCst);
    // A new latch invalidates whatever lane closed the previous one; leaving the old close in
    // place would attribute this submit's fate to another submit's lane.
    LATCH_LANE.store(SAVE_LANE_NONE, Ordering::SeqCst);
    LATCH_LANE_RETURN.store(SUBMIT_RETURN_UNOBSERVED, Ordering::SeqCst);
    LATCH_LANE_STATE.store(SUBMIT_STATE_UNOBSERVED, Ordering::SeqCst);
    if answer & 0xff == 0 {
        let stranded = SUBMIT_LATCHES_WITHOUT_ACCEPT.fetch_add(1, Ordering::SeqCst) + 1;
        // Always logged. A submit that owns the SL device and told its lane "no" is the exact
        // shape four rounds of writer hunting could not see, and it is rare enough that a
        // throttle would be a way of missing it.
        log_message(format_args!(
            "suppress: SUBMIT LATCHED WITHOUT ACCEPTING (#{stranded} of {total} latches) at {} -- \
             iodev+0x10 went 0 -> 0x{:x} across the call and the site returned 0x{answer:x}, so \
             the lane's commit tail cannot run and saveState will never be written to 1. \
             saveState after: {}. Stamp: {}. Device: {}",
            submit_site_label(site),
            after.save_content,
            match state_after {
                Some(state) => state.to_string(),
                None => "unreadable".to_owned(),
            },
            match stamp {
                ELAPSED_MS_UNAVAILABLE => "unstamped (no clock sink wired)".to_owned(),
                ms => format!("+{ms}ms"),
            },
            describe_slot(Some(after))
        ));
        publish_snapshot();
    } else if should_report(total, false) {
        log_message(format_args!(
            "suppress: submit latch #{total} at {} -- iodev+0x10 = 0x{:x}, iodev+0x20 = 0x{:x}, \
             site returned 0x{answer:x}. Stamp: {}",
            submit_site_label(site),
            after.save_content,
            after.job,
            match stamp {
                ELAPSED_MS_UNAVAILABLE => "unstamped".to_owned(),
                ms => format!("+{ms}ms"),
            }
        ));
    }
}

/// Close the most recent latch with the lane's return and the `saveState` it left behind.
///
/// Called from `observe_dispatch` when a latch happened inside the lane call it just forwarded --
/// which is how "did its lane accept" is measured rather than inferred. It records, never repairs.
#[cfg(windows)]
fn note_lane_closed_latch(lane: usize, lane_return: usize) {
    let state = read_save_state();
    LATCH_LANE.store(lane, Ordering::SeqCst);
    LATCH_LANE_RETURN.store(lane_return as u64, Ordering::SeqCst);
    LATCH_LANE_STATE.store(state.unwrap_or(SUBMIT_STATE_UNOBSERVED), Ordering::SeqCst);
    if lane_accepted(lane_return as u64, state.unwrap_or(SUBMIT_STATE_UNOBSERVED)) == Some(false) {
        // The other half of the finding, and the half the builder alone cannot show: the builder
        // said yes and the lane still did not commit.
        log_message(format_args!(
            "suppress: LANE DID NOT COMMIT a submit that latched the SL device -- lane {lane} \
             returned 0x{lane_return:x} and GameMan.saveState is {} after it, with iodev+0x10 = \
             0x{:x} still resident. Nothing will poll this request.",
            match state {
                Some(state) => state.to_string(),
                None => "unreadable".to_owned(),
            },
            LATCH_CONTENT.load(Ordering::SeqCst)
        ));
        publish_snapshot();
    }
}

/// Freeze the latch record against the wedge's `iodev+0x10`, on the wedge's first occurrence.
///
/// Called from [`note_wedged_dispatch`] beside [`snapshot_writers_at_wedge`], on the same
/// first-wins sample, so the attribution describes the birth rather than the plateau.
#[cfg(windows)]
fn snapshot_latch_at_wedge(wedge_content: usize) {
    let observed = latch_content();
    let code = classify_wedge_latch(wedge_content, observed);
    WEDGE_LATCH_MATCH.store(code, Ordering::SeqCst);
    WEDGE_LATCH_CONTENT.store(observed.unwrap_or(0), Ordering::SeqCst);
    // Only a match may publish the submit's own fields; on a mismatch they belong to a different
    // request, and copying them across is exactly how a wedge gets attributed to the wrong submit.
    if code == LATCH_MATCH_SAME {
        WEDGE_LATCH_SITE.store(LATCH_SITE.load(Ordering::SeqCst), Ordering::SeqCst);
        WEDGE_LATCH_BUILDER_RETURN.store(LATCH_BUILDER_RETURN.load(Ordering::SeqCst), Ordering::SeqCst);
        WEDGE_LATCH_LANE_RETURN.store(LATCH_LANE_RETURN.load(Ordering::SeqCst), Ordering::SeqCst);
        WEDGE_LATCH_LANE_STATE.store(LATCH_LANE_STATE.load(Ordering::SeqCst), Ordering::SeqCst);
        WEDGE_LATCH_MS.store(LATCH_MS.load(Ordering::SeqCst), Ordering::SeqCst);
    }
    log_message(format_args!(
        "suppress: WEDGE LATCH ATTRIBUTION -- wedged iodev+0x10 = 0x{wedge_content:x}, last \
         observed latch = {}, match = {}. Verdict: {}",
        match observed {
            Some(content) => format!("0x{content:x} at {}", submit_site_label(latch_site())),
            None => "none observed".to_owned(),
        },
        latch_match_label(code),
        submit_latch_verdict()
    ));
}

#[cfg(windows)]
static ORIG_SUBMIT_COMBINED_BUILDER: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SUBMIT_CHAR_BUILDER: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SUBMIT_ENQUEUE: AtomicUsize = AtomicUsize::new(0);

/// `FUN_140e6ef60`'s real shape, checked at the call site rather than taken from the decompiler.
///
/// Ghidra types the callee with five parameters; `FUN_14067b940` passes six --
/// `mov [rsp+0x28], r12d` and `mov dword [rsp+0x20], 0xa` sit immediately before the `call` in both
/// images. The callee only reads the fifth (`param_5 == 10` is one of its five preconditions), so
/// a five-argument detour would have worked by luck; forwarding both stack slots means the
/// trampoline sees the frame the game built, which is the only version of "unchanged" worth having.
#[cfg(windows)]
type SubmitCombinedBuilderFn =
    unsafe extern "system" fn(usize, usize, u32, usize, u32, u32) -> usize;

/// Observe `FUN_140e6ef60`, the combined lane's submit builder. Forwards all six arguments and the
/// full return value; writes nothing.
///
/// # Safety
/// Installed only on an address decoded from `FUN_14067b940`'s own `call` and audited by
/// `write_site_is_sound`; the signature is the call site's, verified in both images.
#[cfg(windows)]
unsafe extern "system" fn submit_combined_builder_observer(
    iodev: usize,
    char_buffer: usize,
    slot: u32,
    system_buffer: usize,
    opcode: u32,
    flag: u32,
) -> usize {
    let raw = ORIG_SUBMIT_COMBINED_BUILDER.load(Ordering::SeqCst);
    if raw == 0 {
        // Unreachable once bound. Returning 0 here would refuse the user's save, so say so.
        log_message(format_args!(
            "suppress: BUG -- combined submit builder observer ran with no trampoline; the save \
             was NOT built"
        ));
        return 0;
    }
    SUBMIT_COMBINED_CALLS.fetch_add(1, Ordering::Relaxed);
    let before = read_sl_slot();
    let original: SubmitCombinedBuilderFn = unsafe { core::mem::transmute(raw) };
    let answer = unsafe { original(iodev, char_buffer, slot, system_buffer, opcode, flag) };
    note_submit_call(SUBMIT_SITE_COMBINED_BUILDER, before, answer);
    answer
}

/// Observe `FUN_140e6ec70`, the CHARACTER/SYSTEM builder. Four register arguments, verified at all
/// three of its call sites; the union's shape is exact for it.
///
/// # Safety
/// Installed only on an address decoded from three independent call sites that agreed, and audited
/// by `write_site_is_sound`.
#[cfg(windows)]
unsafe extern "system" fn submit_char_builder_observer(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let raw = ORIG_SUBMIT_CHAR_BUILDER.load(Ordering::SeqCst);
    if raw == 0 {
        log_message(format_args!(
            "suppress: BUG -- char submit builder observer ran with no trampoline; the save was \
             NOT built"
        ));
        return 0;
    }
    SUBMIT_CHAR_CALLS.fetch_add(1, Ordering::Relaxed);
    let before = read_sl_slot();
    let original: UnionFn = unsafe { core::mem::transmute(raw) };
    let answer = unsafe { original(a, b, c, d) };
    note_submit_call(SUBMIT_SITE_CHAR_BUILDER, before, answer);
    answer
}

/// Observe `FUN_140e6fb50`, the enqueue -- the only writer of `iodev+0x20`.
///
/// It runs inside a builder, after that builder has already written `iodev+0x10`, so its "before"
/// sample almost always shows the content already latched and [`note_submit_call`] returns without
/// attributing anything. That is correct: this site's value is coverage of the builders that are
/// not hooked (the preview lane's `FUN_140e6ec80`), where it is the only observer that will see the
/// request at all.
///
/// # Safety
/// `SL_ENQUEUE_SAVE_JOB_RVA` is an audited ledger row; the union resolves it once.
#[cfg(windows)]
unsafe extern "system" fn submit_enqueue_observer(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let raw = ORIG_SUBMIT_ENQUEUE.load(Ordering::SeqCst);
    if raw == 0 {
        log_message(format_args!(
            "suppress: BUG -- submit enqueue observer ran with no trampoline; the save job was \
             NOT enqueued"
        ));
        return 0;
    }
    SUBMIT_ENQUEUE_CALLS.fetch_add(1, Ordering::Relaxed);
    let before = read_sl_slot();
    let original: UnionFn = unsafe { core::mem::transmute(raw) };
    let answer = unsafe { original(a, b, c, d) };
    note_submit_call(SUBMIT_SITE_ENQUEUE, before, answer);
    answer
}

/// Decode the callee of one `call rel32` in the running image.
///
/// The containing function is translated through the CALL/READ table (`resolve_call_site_rva`),
/// the offset rides along because the body is `IDENTICAL-WHOLE` with identical `.pdata` extents,
/// and then the displacement is read rather than assumed. Every step refuses rather than guesses:
/// a non-`E8` opcode means the offset no longer points at the call and the answer is `None`.
#[cfg(windows)]
fn decode_call_target(lane_rva: usize, call_offset: usize, what: &str) -> Option<usize> {
    let base = er_game_base::mem::game_module_base().ok().filter(|&b| b != 0)?;
    let site_rva = er_game_base::game_build::resolve_call_site_rva(lane_rva, call_offset, what)?;
    let site = base.checked_add(site_rva)?;
    let mut bytes = [0_u8; CALL_REL32_LEN];
    if !unsafe { read_bytes(site, &mut bytes) } {
        return None;
    }
    if bytes[0] != CALL_REL32_OPCODE {
        log_message(format_args!(
            "suppress: {what} -- 0x{site:x} opens 0x{:02x}, not a call rel32; the submit builder \
             cannot be derived from it and this site will not be watched",
            bytes[0]
        ));
        return None;
    }
    let displacement = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as isize;
    let next = site.checked_add(CALL_REL32_LEN)?;
    let target = if displacement >= 0 {
        next.checked_add(displacement as usize)?
    } else {
        next.checked_sub(displacement.unsigned_abs())?
    };
    er_game_base::game_build::is_game_image_address(target).then_some(target)
}

/// Derive the CHARACTER/SYSTEM builder from all three of its callers and require agreement.
///
/// One decode cannot check itself: a wrong offset that happened to land on some other `E8` yields a
/// plausible address with nothing to contradict it. Three independent lanes calling the same
/// function is a fact about the image, so three decodes that agree is evidence, and a disagreement
/// is a refusal rather than a majority vote.
#[cfg(windows)]
fn derive_char_builder() -> Option<usize> {
    let anchors = [
        (
            SUBMIT_LANE_CHAR_RVA,
            SUBMIT_LANE_CHAR_CALL_OFFSET,
            "char submit builder via FUN_14067b750",
        ),
        (
            SUBMIT_LANE_SYSTEM_RVA,
            SUBMIT_LANE_SYSTEM_CALL_OFFSET,
            "char submit builder via FUN_14067b570",
        ),
        (
            SUBMIT_LANE_ENTRY0B_RVA,
            SUBMIT_LANE_ENTRY0B_CALL_OFFSET,
            "char submit builder via FUN_14067bc10",
        ),
    ];
    let mut agreed: Option<usize> = None;
    for (lane, offset, what) in anchors {
        let Some(target) = decode_call_target(lane, offset, what) else {
            continue;
        };
        match agreed {
            None => agreed = Some(target),
            Some(previous) if previous == target => {}
            Some(previous) => {
                log_message(format_args!(
                    "suppress: submit builder derivation DISAGREES -- {what} decodes 0x{target:x} \
                     but an earlier caller decoded 0x{previous:x}; refusing to hook either, so \
                     the character/system submit path stays unwatched this run"
                ));
                return None;
            }
        }
    }
    agreed
}

/// Chain the three submit observers. Never fatal: a failure costs attribution, not saving.
///
/// The two builders are runtime-derived (`decode_call_target` above), so they go through the
/// entry points that audit against the running image's own `.pdata` instead of the 1.16.2
/// translation table, which has no row for either. The enqueue is an audited ledger row and takes
/// the ordinary translating path.
#[cfg(windows)]
pub fn install_save_submit_latch() {
    if let Some(target) = decode_call_target(
        SUBMIT_LANE_COMBINED_RVA,
        SUBMIT_LANE_COMBINED_CALL_OFFSET,
        "combined submit builder via FUN_14067b940",
    ) {
        // A bare hook, not the union: `FUN_140e6ef60` takes six arguments and the union's shared
        // shape forwards four, which would leave the callee's two stack operands -- including the
        // `param_5 == 10` precondition -- reading whatever the dispatcher's frame happened to hold.
        // That is not an observer, it is a save-breaking rewrite of the arguments.
        match unsafe {
            MhHook::new_runtime_derived(
                target as *mut c_void,
                submit_combined_builder_observer as *mut c_void,
            )
        } {
            Ok(hook) => {
                ORIG_SUBMIT_COMBINED_BUILDER.store(hook.trampoline() as usize, Ordering::SeqCst);
                match unsafe { hook.queue_enable() }.and_then(|()| match unsafe { MH_ApplyQueued() }
                {
                    MH_STATUS::MH_OK => Ok(()),
                    status => Err(status),
                }) {
                    Ok(()) => {
                        SUBMIT_SITES_INSTALLED.fetch_add(1, Ordering::SeqCst);
                        log_message(format_args!(
                            "suppress: submit observer on the COMBINED builder FUN_140e6ef60 \
                             (0x{target:x}, derived from FUN_14067b940+0x{:x}) -- observer only, \
                             all six arguments forwarded",
                            SUBMIT_LANE_COMBINED_CALL_OFFSET
                        ));
                    }
                    Err(status) => {
                        ORIG_SUBMIT_COMBINED_BUILDER.store(0, Ordering::SeqCst);
                        log_message(format_args!(
                            "suppress: combined submit observer enable failed: {status:?}; a latch \
                             from that builder will stay unattributed this run"
                        ));
                    }
                }
            }
            Err(status) => log_message(format_args!(
                "suppress: combined submit observer MhHook::new_runtime_derived(0x{target:x}) \
                 failed: {status:?}; a latch from that builder will stay unattributed this run"
            )),
        }
    }

    if let Some(target) = derive_char_builder() {
        match unsafe {
            er_hook::register_union_hook_runtime_derived(
                target,
                submit_char_builder_observer as UnionFn,
                &ORIG_SUBMIT_CHAR_BUILDER,
            )
        } {
            Ok(()) => {
                SUBMIT_SITES_INSTALLED.fetch_add(1, Ordering::SeqCst);
                log_message(format_args!(
                    "suppress: submit observer on the CHARACTER/SYSTEM builder FUN_140e6ec70 \
                     (0x{target:x}, agreed by three call sites) -- observer only"
                ));
            }
            Err(status) => log_message(format_args!(
                "suppress: char submit observer failed on 0x{target:x}: {status:?}; a latch from \
                 that builder will stay unattributed this run"
            )),
        }
    }

    // Non-resolving on purpose (`game_rva_for_hook`): `register_union_hook` owns the single
    // resolve, and resolving twice can land the detour on a third function in silence.
    match er_game_base::mem::game_rva_for_hook(SL_ENQUEUE_SAVE_JOB_RVA as u32) {
        Ok(address) => match unsafe {
            register_union_hook(
                address,
                submit_enqueue_observer as UnionFn,
                &ORIG_SUBMIT_ENQUEUE,
            )
        } {
            Ok(()) => {
                SUBMIT_SITES_INSTALLED.fetch_add(1, Ordering::SeqCst);
                log_message(format_args!(
                    "suppress: submit observer chained on the ENQUEUE FUN_140e6fb50 \
                     (0x{address:x}) -- observer only"
                ));
            }
            // Expected when suppression is ARMED: `install` binds a bare MinHook detour on this
            // same prologue, and MinHook answers ALREADY_CREATED to the union. Said plainly
            // rather than counted as an install, so `sites_installed` stays honest.
            Err(status) => log_message(format_args!(
                "suppress: submit enqueue observer failed on 0x{address:x}: {status:?} (expected \
                 when suppression is armed, which binds its own detour there); iodev+0x20 writes \
                 will stay unattributed this run"
            )),
        },
        Err(error) => log_message(format_args!(
            "suppress: submit enqueue observer could not resolve FUN_140e6fb50: {error}; \
             iodev+0x20 writes will stay unattributed this run"
        )),
    }
}

// The rules this file adds are decisions about what a set of samples means, not readings of a
// runtime value, so they are exercised with no game attached -- the same reason
// `poll_abandoned_a_save` and `dispatch_sample_is_wedged` are.
#[cfg(test)]
mod save_submit_latch_tests {
    use super::*;

    /// The accept rule, and every way of not claiming one. It is the single question the whole
    /// instrument was built to answer, so it is the one that must be checkable offline.
    #[test]
    fn a_lane_accepts_only_by_returning_non_zero_and_leaving_the_mutex_taken() {
        // The healthy case: the commit tail ran and stamped the mutex.
        assert_eq!(
            lane_accepted(1, GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(true)
        );
        // The wedge SHAPE: the lane said yes but the mutex is still free, so nothing will poll the
        // device it just latched.
        assert_eq!(lane_accepted(1, GAME_MAN_SAVE_STATE_IDLE), Some(false));
        // A refusal, whatever the mutex says afterwards.
        assert_eq!(lane_accepted(0, GAME_MAN_SAVE_STATE_IDLE), Some(false));
        assert_eq!(
            lane_accepted(0, GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(false)
        );
        // The game only tests AL, so only AL may decide it: a return whose low byte is zero is a
        // refusal however much rubbish rides in the upper bits of RAX.
        assert_eq!(
            lane_accepted(0xdead_beef_0000_0000, GAME_MAN_SAVE_STATE_SAVE_OWNS),
            Some(false)
        );
        // A load owning the mutex is not this save being accepted.
        assert_eq!(
            lane_accepted(1, GAME_MAN_SAVE_STATE_LOAD_OWNS),
            Some(false)
        );
        // Unmeasured is a third answer, never a "no".
        assert_eq!(
            lane_accepted(SUBMIT_RETURN_UNOBSERVED, GAME_MAN_SAVE_STATE_SAVE_OWNS),
            None
        );
        assert_eq!(lane_accepted(1, SUBMIT_STATE_UNOBSERVED), None);
    }

    /// Attribution must refuse to describe a submit it did not see, because a wedge blamed on the
    /// wrong request is worse than a wedge with no name.
    #[test]
    fn a_wedge_is_only_attributed_to_the_latch_that_installed_its_own_content() {
        assert_eq!(
            classify_wedge_latch(0x9c9a_1580, Some(0x9c9a_1580)),
            LATCH_MATCH_SAME
        );
        assert_eq!(
            classify_wedge_latch(0x9c9a_1580, Some(0x9ad1_d280)),
            LATCH_MATCH_DIFFERENT
        );
        assert_eq!(
            classify_wedge_latch(0x9c9a_1580, None),
            LATCH_MATCH_NO_OBSERVATION
        );
        // Every code has its own name, or the one thing this produces -- an answer -- is wasted.
        let labels: Vec<&str> = [
            LATCH_MATCH_UNRECORDED,
            LATCH_MATCH_NO_OBSERVATION,
            LATCH_MATCH_SAME,
            LATCH_MATCH_DIFFERENT,
        ]
        .into_iter()
        .map(latch_match_label)
        .collect();
        for (i, label) in labels.iter().enumerate() {
            assert!(!labels[..i].contains(label), "duplicate label {label}");
        }
    }

    /// The verdict must never read as a finding when nothing was watching, and must separate the
    /// two findings from the two ways of having none.
    #[test]
    fn the_verdict_puts_a_missing_instrument_ahead_of_every_other_reading() {
        // Nothing installed beats everything, including a recorded match.
        assert!(
            wedge_latch_verdict(0, LATCH_MATCH_SAME, 0, 0, GAME_MAN_SAVE_STATE_IDLE)
                .contains("no submit site installed")
        );
        // A run that simply behaved must not read as evidence.
        assert!(
            wedge_latch_verdict(
                3,
                LATCH_MATCH_UNRECORDED,
                SUBMIT_RETURN_UNOBSERVED,
                SUBMIT_RETURN_UNOBSERVED,
                SUBMIT_STATE_UNOBSERVED
            )
            .contains("nothing to attribute")
        );
        // The finding, first shape: the builder itself refused after latching.
        assert!(
            wedge_latch_verdict(3, LATCH_MATCH_SAME, 0, 0, GAME_MAN_SAVE_STATE_IDLE)
                .contains("THE FINDING")
        );
        // The finding, second shape: the builder accepted and the lane did not commit.
        assert!(
            wedge_latch_verdict(3, LATCH_MATCH_SAME, 1, 1, GAME_MAN_SAVE_STATE_IDLE)
                .contains("THE FINDING")
        );
        // Not a finding: the submit was accepted all the way through, so the wedge needs a writer
        // after all and the search goes back to the writer set.
        let accepted = wedge_latch_verdict(
            3,
            LATCH_MATCH_SAME,
            1,
            1,
            GAME_MAN_SAVE_STATE_SAVE_OWNS,
        );
        assert!(!accepted.contains("THE FINDING"));
        assert!(accepted.contains("back to the writer set"));
        // Unattributed content must say so rather than borrow another submit's fields.
        assert!(
            wedge_latch_verdict(3, LATCH_MATCH_DIFFERENT, 1, 1, GAME_MAN_SAVE_STATE_SAVE_OWNS)
                .contains("DIFFERENT submit")
        );
        assert!(
            wedge_latch_verdict(
                3,
                LATCH_MATCH_NO_OBSERVATION,
                SUBMIT_RETURN_UNOBSERVED,
                SUBMIT_RETURN_UNOBSERVED,
                SUBMIT_STATE_UNOBSERVED
            )
            .contains("NOT ONE latch was observed")
        );
        // A latch nobody's lane closed is unmeasured, not a verdict either way.
        assert!(
            wedge_latch_verdict(
                3,
                LATCH_MATCH_SAME,
                1,
                SUBMIT_RETURN_UNOBSERVED,
                SUBMIT_STATE_UNOBSERVED
            )
            .contains("unmeasured")
        );
    }

    /// Every field here must read as absent before anything runs, never as a measured zero -- the
    /// property the writer block carries, restated for the fields this one adds. A control run
    /// proved these read as absent rather than as zero and that has to survive.
    #[test]
    fn an_unobserved_submit_set_reads_as_absent_not_as_zero() {
        assert_eq!(submit_sites_installed(), 0);
        assert_eq!(latch_content(), None);
        assert_eq!(latch_site(), SUBMIT_SITE_NONE);
        assert_eq!(latch_builder_return(), SUBMIT_RETURN_UNOBSERVED);
        assert_eq!(latch_builder_state(), SUBMIT_STATE_UNOBSERVED);
        assert_eq!(latch_lane(), SAVE_LANE_NONE);
        assert_eq!(latch_lane_return(), SUBMIT_RETURN_UNOBSERVED);
        assert_eq!(latch_lane_state(), SUBMIT_STATE_UNOBSERVED);
        assert_eq!(latch_ms(), ELAPSED_MS_UNAVAILABLE);
        assert_eq!(wedge_latch_match(), LATCH_MATCH_UNRECORDED);
        assert_eq!(wedge_latch_content(), None);
        assert_eq!(wedge_lane_accepted(), None);
        // The sentinels are not values the fields can hold.
        assert_ne!(SUBMIT_STATE_UNOBSERVED, GAME_MAN_SAVE_STATE_IDLE);
        assert_ne!(SUBMIT_STATE_UNOBSERVED, GAME_MAN_SAVE_STATE_SAVE_OWNS);
        assert_ne!(SUBMIT_STATE_UNOBSERVED, GAME_MAN_SAVE_STATE_LOAD_OWNS);
        assert_ne!(SUBMIT_STATE_UNOBSERVED, GAME_MAN_SAVE_STATE_LOAD_RESIDENT);
        // ...and the verdict says it is silent for lack of an instrument.
        assert!(submit_latch_verdict().contains("no submit site installed"));
    }

    /// The three sites must be distinguishable from each other and from "none", or the answer this
    /// instrument produces -- a name -- cannot be read.
    #[test]
    fn every_submit_site_has_its_own_label() {
        let labels: Vec<&str> = [
            SUBMIT_SITE_NONE,
            SUBMIT_SITE_COMBINED_BUILDER,
            SUBMIT_SITE_CHAR_BUILDER,
            SUBMIT_SITE_ENQUEUE,
        ]
        .into_iter()
        .map(submit_site_label)
        .collect();
        for (i, label) in labels.iter().enumerate() {
            if i > 0 {
                assert_ne!(*label, "none", "site {i} fell through to the unknown label");
            }
            assert!(!labels[..i].contains(label), "duplicate label {label}");
        }
    }
}
