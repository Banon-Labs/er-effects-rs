//! Save-write suppression: swallow the SL submit, report the save as succeeded.
//!
//! # What this does, in one sentence
//!
//! It stops ELDEN RING from ever *enqueueing* a save-write job, and then answers the
//! game's own "did my save finish?" question with the code that means SUCCESS -- so no
//! byte is ever written, no backup is copied or deleted, and every native observer of
//! the save lifecycle sees exactly the state a real successful save leaves behind.
//!
//! # Shared core, one host DLL per process
//!
//! This crate is the suppression core linked by BOTH the standalone
//! `er-save-disable-dll` (census/proof DLL) and the product `er-effects-rs` cdylib
//! (save-game-flow WP1). The host DLL wires the seams before `install`:
//!
//! - [`set_log_sink`]: where human-readable lines go (the standalone's
//!   `er-save-disable.log`, the product's autoload debug log).
//! - [`set_publish_sink`]: called on the telemetry-publish schedule (install,
//!   first-of-each-counter, milestones, every failure path). The standalone wires its
//!   census snapshot writer through the witness reentrancy guard; the product wires a
//!   no-op because its periodic telemetry writer exports the counters on its own cadence.
//!
//! NEVER load `er_save_disable.dll` alongside `er_effects_rs.dll` in one me3 profile:
//! each carries its own MinHook instance and both would detour `0x140e6fb50` /
//! `0x140e6e430`, corrupting each other's trampolines.
//!
//! # The one-shot bypass (save-game-flow WP1)
//!
//! With suppression global, the product needs exactly one sanctioned writer: the
//! System->Quit "Save Game" row. [`arm_one_save_bypass`] arms a single-use token; the
//! next SL save enqueue consumes it and is forwarded to the real trampoline (real
//! submit, real write). Everything else keeps being swallowed.
//!
//! That bypassed save then has to be OBSERVED to completion, and there are three ways it
//! can end, all of them funnelled into the one latch [`take_bypass_final_status`] reads:
//!
//!   * the SL worker's job body returns -- [`adopt_completed_save_job_as_final_status`],
//!     the signal that needs nobody to poll and therefore always exists;
//!   * a native poll consumer answers first (`DoSaveStuff` / the "saving..." MenuJob);
//!   * the native enqueue refuses the submit, which is terminal on the spot.
//!
//! [`bypass_final_status_source`] says which of them it was, so a commit that only ended
//! because the caller's watchdog expired stays distinguishable from one that was observed.
//!
//! # Why this layer and not another (1.16.2, all addresses byte-verified)
//!
//! Every save in the game is request-based and asynchronous. A trigger sets
//! `GameMan+0xb72`/`+0xb73`; a per-frame dispatcher serializes the data and hands it to
//! the platform save-IO device ("SL"); an SL worker thread later opens the file and
//! writes it. The whole write side funnels through a single call:
//!
//! ```text
//!   FUN_14067b750 (game save)     -> FUN_140e6ec70 -+
//!   FUN_14067b940 (game+system)   -> FUN_140e6ef60  |
//!   FUN_14067b570 (system only)   -> FUN_140e6ec70  +-> FUN_140e6fb50 -> FUN_14240ae10
//!   FUN_14067b4e0 (all blocks)    -> FUN_140e6ec80  |      (enqueue)      -> FUN_14240e6f0
//!   FUN_140e6e430 (deferred)      -> FUN_140e6f370 -+                        (worker queue)
//! ```
//!
//! `FUN_140e6fb50` has exactly five callers image-wide (the five above) and is the
//! *only* caller of `FUN_14240ae10`, which is in turn the only caller of the write
//! enqueue `FUN_14240e6f0`. So `FUN_140e6fb50` is not "a good place to intercept" --
//! it is the unique, provable choke point for every save write the game can perform,
//! including the boot-time system-slot save that no trigger-level hook would have seen.
//!
//! It is also strictly *above* the thread hand-off: `CopyFileW` of `ER0000.sl2.bak`
//! (`FUN_142410830`), the `.bak` delete, the BND4 rebuild (`FUN_142413860`), the
//! per-block writes (`FUN_1424142e0`) and `SetEndOfFile` all live inside the job body
//! `FUN_14240fd70`, which only ever runs after a successful enqueue. Swallowing the
//! enqueue therefore removes 100% of the save file IO, not just the payload write.
//!
//! # Why loads are untouched
//!
//! The SL device keeps *save* state in `iodev+0x10` (an `SLSaveContent`) and *load*
//! state in `iodev+0x18` (a distinct 0x230-byte content object). Loads submit through
//! `FUN_140e6eb80` -> `FUN_14240ad30` -> `FUN_14240e420` -- a different enqueue with a
//! different job class, reached from `FUN_14067b200` (slot load), `FUN_14067b1a0`,
//! `FUN_14067b480` and `FUN_140829f30`. None of them touches `FUN_140e6fb50`. Continue
//! and Load Game read the real save file exactly as they always did.
//!
//! # Why the status lie cannot corrupt anything
//!
//! `FUN_140e6e430` (the save status poll) returns the literal `4` from exactly one
//! place: an early-out taken when `iodev+0x10 == 0`, i.e. when *no save request object
//! exists at all*. Every other return path goes through the job-state jump table and
//! can only produce 0,1,2,7,8,9. So the detour calls the original first and only
//! rewrites a `4`. A `4` observed by the original is proof that there is no in-flight
//! save to lie about -- the guard is structural, not a heuristic, and needs no struct
//! offsets, no game-state reads and no timing window.
//!
//! Natively a `4` means "nothing was submitted", which `DoSaveStuff` maps to a silent
//! no-op that never advances `GameMan+0xbc4` -- and the System->Quit menu chain
//! *spins forever waiting for `bc4 == 3`*. Rewriting it to `0` is what closes that
//! deadlock: `0` is the full-success arm.
//!
//! # What the status lie can and cannot do (a hypothesis this file had to kill)
//!
//! It is tempting to blame a Save Game that never wrote on the lie -- "we told the game a
//! save is in progress, so it declined to start another". The decompile rules that out.
//! The lie only ever rewrites `4` (no request) to `0` (SUCCESS); it never produces `1`
//! (in flight). The gate that decides whether a new save may be dispatched is
//! `FUN_14067a080`, which is exactly `GameMan.saveState == 0` -- and the only writers of
//! `saveState` on the poll path, `FUN_140679510` and `FUN_1406794b0`, set it to **0**
//! whenever the status is not `1`. So the lie can only ever OPEN that gate, never close
//! it. When a fired request sits latched with `saveState == 0` and no submit appears, the
//! refusal is downstream of the gate and upstream of the enqueue -- see the save-dispatch
//! observers below, which exist to name which link it was.
//!
//! # The state a swallowed save leaves behind
//!
//! Because the detour returns "submitted OK", the dispatcher runs its real commit tail:
//! `b72 = 0`, `b73 = 0`, `b80 = 1`, `bb8 = 1`, `bbc` bumped, `bc4 1 -> 2`. The next
//! frame `FUN_140679510` polls, gets our `0`, retires `b80 -> 0`, consumes `bb8`,
//! increments `bc0`; `DoSaveStuff` takes case 0 and calls `FUN_14067a980`, which moves
//! `bc4 2 -> 3` and sets the `0x143b355c8` "save concluded" latch. The finalize case-7
//! gate then passes on its own (`b80 == 0`, `!ShouldSave()`, `!FUN_140679370()`), the
//! "saving..." MenuJob reads `0` and reports Success, and the autosave spinner retires
//! within one `CSFeManImp::Update`. No field is forged and no state is poked.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[cfg(windows)]
use std::ffi::c_void;

#[cfg(windows)]
use er_game_base::mem::{game_rva, read_bytes};
#[cfg(windows)]
use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook, UnionFn, register_union_hook};

// ============================================================================
// HOST-DLL SEAMS. The core has no log file and no telemetry file of its own; the one
// DLL that links it installs both sinks before `install`. Same fn-pointer-in-atomic
// pattern as `er_hook::set_hook_logger`. Uninstalled sinks are silent no-ops so the
// pure decision logic stays host-testable with no wiring.
// ============================================================================

/// Signature of the human-log sink: receives `format_args!` output, one line per call.
pub type LogSinkFn = fn(std::fmt::Arguments<'_>);
/// Signature of the telemetry-publish sink: called on the publish schedule (install,
/// first-of-each-counter, milestones, every failure path). The sink owns snapshot
/// serialization and any reentrancy guard it needs.
pub type PublishSinkFn = fn();

static LOG_SINK: AtomicUsize = AtomicUsize::new(0);
static PUBLISH_SINK: AtomicUsize = AtomicUsize::new(0);

/// Install the human-log sink. Call once, before [`install`], so no install/verify line
/// is ever dropped.
pub fn set_log_sink(sink: LogSinkFn) {
    LOG_SINK.store(sink as usize, Ordering::Release);
}

/// Install the telemetry-publish sink. Call once, before [`install`]. A host whose own
/// telemetry writer already exports the counters on a periodic cadence may install a
/// no-op.
pub fn set_publish_sink(sink: PublishSinkFn) {
    PUBLISH_SINK.store(sink as usize, Ordering::Release);
}

fn log_message(args: std::fmt::Arguments<'_>) {
    let raw = LOG_SINK.load(Ordering::Acquire);
    if raw != 0 {
        // SAFETY: `raw` is only ever a `LogSinkFn` stored by `set_log_sink`.
        let sink: LogSinkFn = unsafe { std::mem::transmute::<usize, LogSinkFn>(raw) };
        sink(args);
    }
}

/// Publish a telemetry snapshot through the host sink (no-op until one is installed).
fn publish_snapshot() {
    let raw = PUBLISH_SINK.load(Ordering::Acquire);
    if raw != 0 {
        // SAFETY: `raw` is only ever a `PublishSinkFn` stored by `set_publish_sink`.
        let sink: PublishSinkFn = unsafe { std::mem::transmute::<usize, PublishSinkFn>(raw) };
        sink();
    }
}

/// `FUN_140e6fb50` -- allocates the SL job wrapper and pushes it onto the save-IO
/// worker queue. Returns `bool` in AL: true = submitted.
#[cfg(windows)]
const SL_ENQUEUE_SAVE_JOB_RVA: usize = 0xe6fb50;
/// `FUN_140e6e430` -- polls the outcome of the outstanding save request.
#[cfg(windows)]
const SL_POLL_SAVE_STATUS_RVA: usize = 0xe6e430;
/// `FUN_140e6f200` -- the device's own request teardown. Releases `iodev+0x10`
/// (save content), `+0x18` (load content), `+0x20` (job) and `+0x28` (file cap)
/// through `CSDelayDeleteMan`/`CSFile`, and zeroes `+0x44`. This is precisely what
/// the native code calls when the enqueue fails, which is the state we synthesize.
#[cfg(windows)]
const SL_RELEASE_REQUEST_RVA: usize = er_game_base::rva::SL_RELEASE_REQUEST_RVA;

/// The status code `FUN_140e6e430` returns when `iodev+0x10 == 0`, meaning "there is
/// no save request". Its single producer is the `MOV EAX,0x4` at `0x140e6e460`.
const SL_STATUS_NO_REQUEST: u32 = 4;
/// The status code that means "the save completed successfully". `DoSaveStuff` maps it
/// to the only arm that advances `GameMan+0xbc4` 2 -> 3.
const SL_STATUS_SUCCESS: u32 = 0;
/// The status code for a still-running save job. The bypass completion watch skips it:
/// the first post-allow poll result that is NOT this value is the terminal outcome.
#[cfg(windows)]
const SL_STATUS_IN_FLIGHT: u32 = 1;

/// Opening bytes of each target as they appear in the 1.16.2 image. Verified identical
/// in the Ghidra 1.16.2 runtime dump and in `eldenring-deobf.bin` at the same VA
/// (shift 0). Checked at install time: if the bytes do not match, the address means
/// something else in this build and the hook is refused rather than crash-installed.
///
/// Note the two-byte `40 53` / `40 57` pushes -- a redundant REX prefix MSVC emits
/// here. Both prologues decode to whole instructions well past MinHook's 5-byte
/// relocation window and contain no relative branches, so they are safe to patch.
const SL_ENQUEUE_SAVE_JOB_SIG: &[u8] = &[
    0x40, 0x53, 0x56, 0x57, 0x48, 0x83, 0xEC, 0x50, 0x48, 0xC7, 0x44, 0x24, 0x30, 0xFE, 0xFF, 0xFF,
    0xFF, 0x8B, 0xF2, 0x48, 0x8B, 0xD9,
];
const SL_POLL_SAVE_STATUS_SIG: &[u8] = &[
    0x40, 0x57, 0x48, 0x83, 0xEC, 0x70, 0x48, 0xC7, 0x44, 0x24, 0x28, 0xFE, 0xFF, 0xFF, 0xFF, 0x48,
    0x89, 0x9C, 0x24, 0x88, 0x00, 0x00, 0x00,
];
/// `FUN_14067a980` -- the ONLY code that moves `GameMan+0xbc4` from 2 to 3, i.e. the
/// moment the quit-to-title wait job is released. Its whole body is
/// `if (bc4 == 2) bc4 = 3;`.
#[cfg(windows)]
const QUIT_PHASE_SETTLE_RVA: usize = 0x67a980;
/// `mov rax,[rip+..]; cmp dword [rax+0xbc4],2; jne`.
const QUIT_PHASE_SETTLE_SIG: &[u8] = &[
    0x48, 0x8B, 0x05, 0x91, 0xEF, 0x6E, 0x03, 0x83, 0xB8, 0xC4, 0x0B, 0x00, 0x00, 0x02, 0x75, 0x0A,
];

const SL_RELEASE_REQUEST_SIG: &[u8] = &[
    0x48, 0x89, 0x6C, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x57, 0x48, 0x83, 0xEC, 0x20, 0x33,
    0xED, 0x48, 0x8B, 0xF9, 0x48, 0x39, 0x69, 0x28,
];

// ============================================================================
// THE SL REQUEST SLOT. The one set of numbers that decides whether a save that never
// reached the enqueue was refused by the submit builder's precondition, and if so by
// which operand.
//
// `FUN_140e6ef60` -- the submit builder the COMBINED lane `FUN_14067b940` calls -- opens
// with a single conjunction (1.16.2 decompile, shift 0):
//
// ```text
//   if (iodev+0x10 == 0 && iodev+0x20 == 0 && buf != 0 && tail != 0
//       && slot < 10 && kind == 10) { ...build... }
//   return 0;                                   // AL = 0: "no submit"
// ```
//
// Its ONLY caller passes `FUN_140e6ef60(FUN_140e6e060(), pvVar4, param_1, buffer, 10, ..)`
// and has already proven four of those six operands:
//
//   * `buf`  (`pvVar4`, the 0x280000 MainHeap block) -- `if (pvVar4 == 0) return 0;`
//   * `tail` (`buffer`, the 0x60000 block)           -- `if (buffer == 0) return 0;`
//   * `slot` (`param_1`)                             -- `if (9 < param_1 || CanShowSaveMenu())`
//                                                       diverts to the system-only lane,
//                                                       so the builder only ever sees < 10
//   * `kind`                                         -- the literal 10 at the call site
//
// So when that lane declines AFTER a successful serialization, the guard can only have
// failed on `iodev+0x10` or `iodev+0x20`. There is no third possibility, and the two are
// different bugs. `FUN_140e6ec70` (the char-only and system-only lanes' builder) gates on
// the same two fields plus `buf != 0`.
//
// The remaining non-guard exit is the `HeapAlloc(0x298)`/`SLSaveContent::SLSaveContent`
// pair; it stores its result into `iodev+0x10` before testing it, so a failure there
// leaves the slot CLEAR and is distinguishable from a guard bail by exactly this sample.
//
// WHY BOTH FIELDS AND NOT JUST `+0x10`: `iodev+0x20` is a SHARED job slot. The LOAD
// builders `FUN_140e6f430`/`FUN_140e6f5b0` write `param_1[4]`, which is `iodev+0x20`, and
// gate on `iodev+0x18 == 0 && iodev+0x20 == 0`. A load that completed but was never
// consumed therefore blocks every subsequent save through the same conjunction -- and
// `FUN_140e6e080` case 0x14 deliberately does NOT release on a successful load, deferring
// that to the consumer `FUN_14067b100` -> `FUN_140e6e380` -> `FUN_140e6f200`. Sampling
// `+0x18` alongside `+0x20` separates "a stale save request" from "a stale load request",
// which the guard itself cannot.
// ============================================================================

/// `DAT_144589390` -- the process-wide SL IO device singleton, the `iodev` every save and
/// load request lives in.
///
/// Read directly rather than by calling `FUN_140e6e060`: the getter lazily constructs the
/// device under a global mutex, and an instrument must never be able to allocate. Proven
/// to be the whole story by xref -- the global has exactly five references image-wide, all
/// inside `FUN_140e6e060` (the getter) and `FUN_140e6f6f0` (the destructor), so every
/// consumer in the game reaches the same object through the getter.
#[cfg(windows)]
const SL_IODEV_GLOBAL_RVA: usize = er_game_base::rva::SL_IODEV_GLOBAL_RVA;

/// Bytes of the device sampled in one `ReadProcessMemory`: enough to cover `+0x30`.
#[cfg(windows)]
const SL_IODEV_SAMPLE_BYTES: usize = 0x38;

/// `iodev+0x10` -- the `SLSaveContent` of an outstanding SAVE request. First operand of
/// the submit builders' precondition.
const SL_IODEV_SAVE_CONTENT_OFFSET: usize = 0x10;
/// `iodev+0x18` -- the load-side content object. Not a save-guard operand, but a non-zero
/// value beside a non-zero `+0x20` identifies the job as a LOAD's.
const SL_IODEV_LOAD_CONTENT_OFFSET: usize = 0x18;
/// `iodev+0x20` -- the in-flight job, SHARED by the save and load lanes. Second operand of
/// the submit builders' precondition.
const SL_IODEV_JOB_OFFSET: usize = 0x20;
/// `iodev+0x28` -- an `FD4FileCap` still loading. Not a guard operand: when it is set the
/// builders DEFER (store the opcode at `+0x30`, return 1) instead of declining, and
/// `FUN_140e6e430` routes the poll to `FUN_140e6f370`.
const SL_IODEV_FILE_CAP_OFFSET: usize = 0x28;
/// `iodev+0x30` -- the opcode parked by a deferred build, replayed by `FUN_140e6f370`.
const SL_IODEV_DEFERRED_OPCODE_OFFSET: usize = 0x30;

/// A sample of the SL device's request slot: the submit builders' precondition operands
/// plus the two fields that say who owns them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlRequestSlot {
    /// `iodev+0x10`, the outstanding save request's content.
    pub save_content: usize,
    /// `iodev+0x18`, the outstanding load request's content.
    pub load_content: usize,
    /// `iodev+0x20`, the in-flight job (save OR load).
    pub job: usize,
    /// `iodev+0x28`, a file capability still being loaded.
    pub file_cap: usize,
    /// `iodev+0x30`, the opcode a deferred build parked for `FUN_140e6f370`.
    pub deferred_opcode: usize,
}

impl SlRequestSlot {
    /// True when the submit builders' `iodev+0x10 == 0 && iodev+0x20 == 0` precondition
    /// holds -- i.e. a save COULD be built from this slot.
    pub fn admits_a_save(&self) -> bool {
        self.save_content == 0 && self.job == 0
    }

    /// True when the LOAD builders' `iodev+0x18 == 0 && iodev+0x20 == 0` precondition holds
    /// -- i.e. no load request occupies the device. `FUN_140e6f430` opens with
    /// `if (param_1[3] != 0 || param_1[4] != 0) return 0;` and `FUN_140e6f5b0` with
    /// `if (param_1[3] == 0 && param_1[4] == 0) { ...build... }`; `param_1[3]`/`[4]` are
    /// `iodev+0x18`/`+0x20`.
    ///
    /// The save side needs this too, because `+0x20` is SHARED: a load that holds the job
    /// fails the save builders' second operand just as surely as a stale save would.
    pub fn admits_a_load(&self) -> bool {
        self.load_content == 0 && self.job == 0
    }
}

/// No decline has been classified yet.
pub const SL_BAIL_UNSAMPLED: usize = 0;
/// The device pointer or its fields could not be read; the sample proves nothing.
pub const SL_BAIL_IODEV_UNREADABLE: usize = 1;
/// `iodev+0x10` still holds an `SLSaveContent`: a previous SAVE request was never released.
pub const SL_BAIL_SAVE_CONTENT_LATCHED: usize = 2;
/// `iodev+0x20` holds a job and `iodev+0x18` holds load content: a completed LOAD was never
/// consumed, and it is occupying the shared job slot the save needs.
pub const SL_BAIL_LOAD_JOB_LATCHED: usize = 3;
/// `iodev+0x20` holds a job with no content on either side -- an orphaned job.
pub const SL_BAIL_JOB_LATCHED: usize = 4;
/// Both guard operands are populated.
pub const SL_BAIL_SAVE_CONTENT_AND_JOB_LATCHED: usize = 5;
/// The precondition HOLDS, so the builder got past its guard: the refusal is the
/// NetworkHeap `SLSaveContent` allocation, or the lane bailed before calling the builder.
pub const SL_BAIL_PRECONDITION_CLEAR: usize = 6;

/// Name the reason code produced by [`classify_sl_bail`].
pub fn sl_bail_reason_label(code: usize) -> &'static str {
    match code {
        SL_BAIL_IODEV_UNREADABLE => "iodev-unreadable",
        SL_BAIL_SAVE_CONTENT_LATCHED => "save-content-latched-0x10",
        SL_BAIL_LOAD_JOB_LATCHED => "load-job-latched-0x18+0x20",
        SL_BAIL_JOB_LATCHED => "orphan-job-latched-0x20",
        SL_BAIL_SAVE_CONTENT_AND_JOB_LATCHED => "save-content-and-job-latched-0x10+0x20",
        SL_BAIL_PRECONDITION_CLEAR => "precondition-clear-builder-alloc-refused",
        _ => "unsampled",
    }
}

/// Say WHICH operand of the submit builders' precondition failed, from one slot sample.
///
/// Pure and total so the mapping is unit-testable on the host with no game attached: the
/// whole point of the instrument is that a single decline names the culprit, and a
/// classifier that is only exercised at runtime cannot be trusted to do that.
pub fn classify_sl_bail(slot: Option<SlRequestSlot>) -> usize {
    let Some(slot) = slot else {
        return SL_BAIL_IODEV_UNREADABLE;
    };
    match (slot.save_content != 0, slot.job != 0) {
        (true, true) => SL_BAIL_SAVE_CONTENT_AND_JOB_LATCHED,
        (true, false) => SL_BAIL_SAVE_CONTENT_LATCHED,
        (false, true) if slot.load_content != 0 => SL_BAIL_LOAD_JOB_LATCHED,
        (false, true) => SL_BAIL_JOB_LATCHED,
        (false, false) => SL_BAIL_PRECONDITION_CLEAR,
    }
}

/// Render a slot sample for a log line: every guard operand, by name, with its value.
///
/// The values matter as much as the verdict. "The precondition failed" is a conclusion; a
/// reader needs the pointers to tell a stale save request from a stale load request, and
/// to compare the same address across two lines.
pub fn describe_slot(slot: Option<SlRequestSlot>) -> String {
    match slot {
        None => "iodev UNREADABLE".to_owned(),
        Some(slot) => format!(
            "+0x10 save_content=0x{:x} +0x18 load_content=0x{:x} +0x20 job=0x{:x} \
             +0x28 file_cap=0x{:x} +0x30 deferred_opcode={} [{}]",
            slot.save_content,
            slot.load_content,
            slot.job,
            slot.file_cap,
            slot.deferred_opcode,
            sl_bail_reason_label(classify_sl_bail(Some(slot)))
        ),
    }
}

/// Read the SL device's request slot, or `None` when the device is not resolvable.
///
/// Two `ReadProcessMemory` calls total (the singleton pointer, then one window over the
/// fields) so it is cheap enough to run on every decline, which repeats every frame while
/// a request stays latched.
#[cfg(windows)]
fn read_sl_slot() -> Option<SlRequestSlot> {
    use er_game_base::mem::safe_read_usize;

    const MIN_PLAUSIBLE_POINTER: usize = 0x10000;

    let global = game_rva(SL_IODEV_GLOBAL_RVA as u32).ok()?;
    let iodev = unsafe { safe_read_usize(global) }?;
    if iodev < MIN_PLAUSIBLE_POINTER {
        return None;
    }
    let mut window = [0_u8; SL_IODEV_SAMPLE_BYTES];
    if !unsafe { read_bytes(iodev, &mut window) } {
        return None;
    }
    let field = |offset: usize| -> usize {
        let mut bytes = [0_u8; core::mem::size_of::<usize>()];
        bytes.copy_from_slice(&window[offset..offset + core::mem::size_of::<usize>()]);
        usize::from_le_bytes(bytes)
    };
    Some(SlRequestSlot {
        save_content: field(SL_IODEV_SAVE_CONTENT_OFFSET),
        load_content: field(SL_IODEV_LOAD_CONTENT_OFFSET),
        job: field(SL_IODEV_JOB_OFFSET),
        file_cap: field(SL_IODEV_FILE_CAP_OFFSET),
        deferred_opcode: field(SL_IODEV_DEFERRED_OPCODE_OFFSET) & 0xffff_ffff,
    })
}

/// Read the SL device pointer alone, for comparing against a detour's `iodev` argument.
#[cfg(windows)]
fn read_sl_iodev() -> Option<usize> {
    use er_game_base::mem::safe_read_usize;

    const MIN_PLAUSIBLE_POINTER: usize = 0x10000;

    let global = game_rva(SL_IODEV_GLOBAL_RVA as u32).ok()?;
    let iodev = unsafe { safe_read_usize(global) }?;
    (iodev >= MIN_PLAUSIBLE_POINTER).then_some(iodev)
}

/// Latch a slot sample into the reporting statics.
#[cfg(windows)]
fn store_slot_sample(slot: Option<SlRequestSlot>, into: &SlotSampleCell) {
    match slot {
        Some(slot) => {
            into.save_content.store(slot.save_content, Ordering::SeqCst);
            into.load_content.store(slot.load_content, Ordering::SeqCst);
            into.job.store(slot.job, Ordering::SeqCst);
            into.file_cap.store(slot.file_cap, Ordering::SeqCst);
            into.readable.store(1, Ordering::SeqCst);
        }
        None => {
            into.readable.store(0, Ordering::SeqCst);
            SLOT_READ_FAILURES.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Storage for one latched slot sample. Four fields plus a readability flag, so a zero
/// can never be mistaken for "we could not read it".
struct SlotSampleCell {
    save_content: AtomicUsize,
    load_content: AtomicUsize,
    job: AtomicUsize,
    file_cap: AtomicUsize,
    readable: AtomicUsize,
}

impl SlotSampleCell {
    const fn new() -> Self {
        Self {
            save_content: AtomicUsize::new(0),
            load_content: AtomicUsize::new(0),
            job: AtomicUsize::new(0),
            file_cap: AtomicUsize::new(0),
            readable: AtomicUsize::new(0),
        }
    }

    fn snapshot(&self) -> Option<SlRequestSlot> {
        if self.readable.load(Ordering::SeqCst) == 0 {
            return None;
        }
        Some(SlRequestSlot {
            save_content: self.save_content.load(Ordering::SeqCst),
            load_content: self.load_content.load(Ordering::SeqCst),
            job: self.job.load(Ordering::SeqCst),
            file_cap: self.file_cap.load(Ordering::SeqCst),
            deferred_opcode: 0,
        })
    }
}

/// The slot as it stood when the lane last DECLINED -- the sample that names the culprit.
static DECLINE_SLOT: SlotSampleCell = SlotSampleCell::new();
/// The slot as it stood immediately BEFORE the last swallow's `FUN_140e6f200` call.
static SWALLOW_SLOT_BEFORE: SlotSampleCell = SlotSampleCell::new();
/// The slot as it stood immediately AFTER it. If this is not clear, our release is the bug.
static SWALLOW_SLOT_AFTER: SlotSampleCell = SlotSampleCell::new();
/// Reason code (`SL_BAIL_*`) for the most recent decline.
static DECLINE_BAIL_REASON: AtomicUsize = AtomicUsize::new(SL_BAIL_UNSAMPLED);
/// Swallows whose release left the builders' precondition still failing. Any non-zero
/// value is a self-inflicted permanent save refusal.
static SWALLOW_RELEASE_LEFT_DIRTY: AtomicU64 = AtomicU64::new(0);
/// Swallows where the `iodev` we were handed was not the singleton we release against.
/// Non-zero would mean the release is being applied to the wrong object.
static SWALLOW_IODEV_MISMATCH: AtomicU64 = AtomicU64::new(0);
/// Slot samples that could not be read at all.
static SLOT_READ_FAILURES: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// THE LOAD CONSUMER. The other half of the shared `iodev+0x20` job slot, and the reason a
// save can be refused forever by something that is not a save at all.
//
// A completed LOAD is NOT released where it completes. `FUN_140e6e080` case 0x14 with a
// zero `param_2` reads the job's result, finds success, and returns 0 having called
// nothing -- deliberately, because the payload has not been handed to anyone yet. The
// release is owed by the CONSUMER:
//
// ```text
//   FUN_14067b100(out_buf, size)          // gate: saveState == 3 && GameMan+0xdf0 == 0
//     -> FUN_140e6e380(iodev, out, size)  // gate: +0x18 && +0x20 && result==0 && state==0x14
//          memcpy(out, content, n)
//          FUN_140e6f200(iodev)           // <-- the release, and its ONLY unconditional site
// ```
//
// So the load's job survives exactly as long as its consumer is not run. And the submit
// builders gate on `iodev+0x10 == 0 && iodev+0x20 == 0`, which the surviving job fails --
// permanently, for every save in the process. That is the shape of a save refusal whose
// cause is a load: [`SL_BAIL_LOAD_JOB_LATCHED`].
//
// Anything that stands in for `FUN_14067b100` therefore inherits its debt to the device.
// The helpers below let such a substitution PROVE that the debt was paid, by sampling the
// slot on both sides of whatever it did and classifying the transition. They deliberately
// do not release anything themselves: releasing from here would mean re-deriving the
// native guard (`FUN_14240a180`/`FUN_14240a1f0` on the job) in our own code, and a guard
// we re-derive is a guard that can disagree with the game about whether a load is still
// in flight. Running the game's own consumer and MEASURING it cannot disagree.
// ============================================================================

/// No load-consumer call has been classified yet.
pub const LOAD_CONSUMER_UNSAMPLED: usize = 0;
/// The device could not be read on one or both sides; the samples prove nothing.
pub const LOAD_CONSUMER_UNREADABLE: usize = 1;
/// The device held no load request going in, so there was no release to owe.
pub const LOAD_CONSUMER_NOTHING_HELD: usize = 2;
/// A load request was held and the slot came back clear: the consumer ran and released it.
pub const LOAD_CONSUMER_RELEASED: usize = 3;
/// A load request was held and is STILL held: the native guard declined, which is what it
/// does while the job has not reached its terminal state. Nothing was taken from a live
/// load -- but if the caller now swallows the read, this request is stranded.
pub const LOAD_CONSUMER_STILL_HELD: usize = 4;

/// Name a [`classify_load_consumer`] outcome.
pub fn load_consumer_outcome_label(code: usize) -> &'static str {
    match code {
        LOAD_CONSUMER_UNREADABLE => "iodev-unreadable",
        LOAD_CONSUMER_NOTHING_HELD => "no-load-request-held",
        LOAD_CONSUMER_RELEASED => "load-request-released",
        LOAD_CONSUMER_STILL_HELD => "load-request-still-held",
        _ => "unsampled",
    }
}

/// Classify what a load-consumer call did to the device, from the slot on each side.
///
/// Pure and total so the mapping is unit-testable on the host: this is the oracle that
/// says whether the shared job slot was freed, and an oracle only exercised at runtime
/// cannot be trusted to say so.
pub fn classify_load_consumer(
    before: Option<SlRequestSlot>,
    after: Option<SlRequestSlot>,
) -> usize {
    let (Some(before), Some(after)) = (before, after) else {
        return LOAD_CONSUMER_UNREADABLE;
    };
    if before.admits_a_load() {
        return LOAD_CONSUMER_NOTHING_HELD;
    }
    if after.admits_a_load() {
        LOAD_CONSUMER_RELEASED
    } else {
        LOAD_CONSUMER_STILL_HELD
    }
}

/// Load-consumer calls observed (one per substituted `FUN_14067b100`).
static LOAD_CONSUMER_CALLS: AtomicU64 = AtomicU64::new(0);
/// Calls after which the shared job slot was free. This is the "the slot was freed, and by
/// the native consumer we invoked" oracle.
static LOAD_CONSUMER_RELEASES: AtomicU64 = AtomicU64::new(0);
/// Calls where the native guard kept the request because the load had not finished. This
/// is the race oracle: it counts the times the guard refused to take a live load away.
static LOAD_CONSUMER_STILL_HELD_COUNT: AtomicU64 = AtomicU64::new(0);
/// Calls where the device could not be sampled.
static LOAD_CONSUMER_UNREADABLE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Calls that ended with a load request still latched AND the caller going on to substitute
/// the payload -- i.e. a request this process has now stranded. Non-zero is the regression
/// signature of the post-switch save refusal: every later save fails `iodev+0x20 == 0`.
static LOAD_CONSUMER_STRANDED: AtomicU64 = AtomicU64::new(0);
/// Outcome code of the most recent load-consumer call.
static LOAD_CONSUMER_LAST_OUTCOME: AtomicUsize = AtomicUsize::new(LOAD_CONSUMER_UNSAMPLED);
/// The slot as it stood immediately BEFORE the last load-consumer call.
static LOAD_CONSUMER_SLOT_BEFORE: SlotSampleCell = SlotSampleCell::new();
/// The slot as it stood immediately AFTER it.
static LOAD_CONSUMER_SLOT_AFTER: SlotSampleCell = SlotSampleCell::new();

/// Sample the SL device's request slot for a caller outside this crate.
///
/// Exposed so the one place that substitutes `FUN_14067b100` can bracket its call without
/// re-deriving the device pointer, the field offsets, or the fault-safe read.
#[cfg(windows)]
pub fn sample_sl_request_slot() -> Option<SlRequestSlot> {
    read_sl_slot()
}

/// Record what running the native load consumer did to the device, and return the outcome.
///
/// `origin` names the substitution site for the log. `payload_substituted` says whether the
/// caller is about to hand the engine different bytes -- when it is, an outcome of
/// [`LOAD_CONSUMER_STILL_HELD`] means a request has just been stranded, which is worth a
/// line every time because from that point on no save in the process can be built.
#[cfg(windows)]
pub fn note_load_consumer(
    before: Option<SlRequestSlot>,
    after: Option<SlRequestSlot>,
    origin: &str,
    payload_substituted: bool,
) -> usize {
    store_slot_sample(before, &LOAD_CONSUMER_SLOT_BEFORE);
    store_slot_sample(after, &LOAD_CONSUMER_SLOT_AFTER);
    let outcome = classify_load_consumer(before, after);
    LOAD_CONSUMER_LAST_OUTCOME.store(outcome, Ordering::SeqCst);
    let calls = LOAD_CONSUMER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    match outcome {
        LOAD_CONSUMER_RELEASED => {
            let releases = LOAD_CONSUMER_RELEASES.fetch_add(1, Ordering::SeqCst) + 1;
            if should_report(releases, false) {
                log_message(format_args!(
                    "suppress: load consumer at {origin} released the shared job slot \
                     (release #{releases} of {calls} calls) -- before {}, after {}; the save \
                     builders' `iodev+0x10 == 0 && iodev+0x20 == 0` precondition is open again",
                    describe_slot(before),
                    describe_slot(after)
                ));
                publish_snapshot();
            }
        }
        LOAD_CONSUMER_STILL_HELD => {
            LOAD_CONSUMER_STILL_HELD_COUNT.fetch_add(1, Ordering::SeqCst);
            if payload_substituted {
                // Unthrottled: this is a latch, not a rate. From here on every save in the
                // process is refused by a job WE left in the shared slot.
                let stranded = LOAD_CONSUMER_STRANDED.fetch_add(1, Ordering::SeqCst) + 1;
                log_message(format_args!(
                    "suppress: BUG -- load consumer at {origin} did NOT release the shared job \
                     slot (#{stranded}) and the payload was substituted anyway; before {}, \
                     after {}. Every later save now fails the submit builders' \
                     `iodev+0x20 == 0` operand",
                    describe_slot(before),
                    describe_slot(after)
                ));
                publish_snapshot();
            } else {
                log_message(format_args!(
                    "suppress: load consumer at {origin} left the request in place -- the \
                     native guard says the job has not finished; slot {}",
                    describe_slot(before)
                ));
            }
        }
        LOAD_CONSUMER_UNREADABLE => {
            LOAD_CONSUMER_UNREADABLE_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
    outcome
}

/// Load-consumer calls observed.
pub fn load_consumer_calls() -> u64 {
    LOAD_CONSUMER_CALLS.load(Ordering::SeqCst)
}

/// Calls that left the shared job slot free.
pub fn load_consumer_releases() -> u64 {
    LOAD_CONSUMER_RELEASES.load(Ordering::SeqCst)
}

/// Calls where the native guard kept a not-yet-finished load. Non-zero proves the guard is
/// load-bearing rather than vacuous; it is the signal to check whether a release raced one.
pub fn load_consumer_still_held() -> u64 {
    LOAD_CONSUMER_STILL_HELD_COUNT.load(Ordering::SeqCst)
}

/// Calls that stranded a load request. Any non-zero value is a permanent save refusal.
pub fn load_consumer_stranded() -> u64 {
    LOAD_CONSUMER_STRANDED.load(Ordering::SeqCst)
}

/// Outcome code of the most recent load-consumer call.
pub fn load_consumer_last_outcome() -> usize {
    LOAD_CONSUMER_LAST_OUTCOME.load(Ordering::SeqCst)
}

/// Name the most recent load-consumer outcome.
pub fn load_consumer_last_outcome_label() -> &'static str {
    load_consumer_outcome_label(LOAD_CONSUMER_LAST_OUTCOME.load(Ordering::SeqCst))
}

/// The slot as it stood before the last load-consumer call.
pub fn load_consumer_slot_before() -> Option<SlRequestSlot> {
    LOAD_CONSUMER_SLOT_BEFORE.snapshot()
}

/// The slot as it stood after the last load-consumer call.
pub fn load_consumer_slot_after() -> Option<SlRequestSlot> {
    LOAD_CONSUMER_SLOT_AFTER.snapshot()
}

// ============================================================================
// SAVE-DISPATCH OBSERVERS. Pure observation of the three native save-dispatch lanes and
// the character serializer that gates two of them. They change nothing; they exist so a
// save that never reaches the enqueue can be attributed in ONE run instead of three.
//
// Why they are needed (1.16.2 decompile, `FUN_140aff640` = MoveMapStep step 18, the
// steady in-world step -- its step-table entry sits at MoveMapStep+0x378, and the table
// base 0xa8 with 0x28-byte entries puts that at index 18, the `STEP_MoveMap` index this
// repo already tracks; the array ends at +0x4b8, which the same function reads):
//
//   FUN_140aff640 (every in-world frame)
//     -> DoSaveStuff              polls the SL status while GameMan.saveState == 1
//     -> FUN_140afb880            the save DISPATCHER
//          gate: !BOOL_143d856a0, (ShouldSave() || b73 || slotLoad != -1),
//                and FUN_14067a080() == (GameMan.saveState == 0)
//          lane: b72 && b73 -> FUN_14067b940   (combined: char slot + system)
//                b72        -> FUN_14067b750   (char slot only)
//                       b73 -> FUN_14067b570   (system only)
//
// The two CHARACTER lanes only build a submit when `FUN_14067dc00` (the character
// serializer) returns non-zero:
//
//   cVar2 = FUN_14067dc00(GameMan, buf, 0x280000, 0);
//   ...
//   if (cVar2 != '\0') { cVar3 = FUN_140e6ef60(iodev, buf, slot, ...); }   // -> the enqueue
//   if (cVar3 != '\0') { bc4 1->2; bb8 = 1; saveState = 1; b72 = b73 = 0; }
//
// So a serializer that returns 0 makes the lane return 0 having touched NOTHING: b72/b73
// stay set, saveState stays 0, the dispatcher re-enters next frame, and no SL enqueue is
// ever created. From the outside that is indistinguishable from "the dispatcher never
// ran" -- and both look identical to a one-shot bypass token, which simply expires.
// These observers make the three cases distinguishable.
//
// The serializer's own first gate is
//   `buf == 0 || GameMan+0xcb1 != 0 || GameMan+0xcb2 != 0 || [0x143d68078] == 0`
// and it is the ONE exit that returns without writing the byte counter `_DAT_143d69920`.
// That gate is PROVEN UNREACHABLE in this scenario, so the byte counter is written on
// every exit we can actually observe -- see `SAVE_SERIALIZE_BYTES_RVA` for the proof and
// `serialize_fail_step_label` for the decode.
// ============================================================================

/// `FUN_14067b940` -- combined save dispatcher, taken when b72 AND b73 are set. This is
/// the lane the Save Game commit deliberately produces (it fires both request setters).
#[cfg(windows)]
const SAVE_DISPATCH_COMBINED_RVA: usize = er_game_base::rva::SAVE_DISPATCH_COMBINED_RVA;
/// `FUN_14067b750` -- character-slot-only dispatcher (b72 set, b73 clear).
///
/// Same address as `SAVE_WRITE_TO_SLOT_RVA` in er-effects-rs and er-save-loader, where it
/// was called `CONTINUE_LOAD_RVA` until 2026-08-01. This crate's name was the correct one:
/// the 1.16.2 dump shows it writes a save (serializes via `SAVE_SERIALIZE_CHAR_RVA` below,
/// then submits through the IO device and sets `saveState = 1`). Deliberately NOT renamed to
/// match the others -- the `_COMBINED` / `_CHAR` / `_SYSTEM` family here encodes the b72/b73
/// lane distinction, which a generic "write to slot" name would lose.
#[cfg(windows)]
const SAVE_DISPATCH_CHAR_RVA: usize = 0x67b750;
/// `FUN_14067b570` -- system-slot-only dispatcher (b73 set, b72 clear). Unlike the two
/// character lanes it does NOT consult `FUN_14067dc00`; it always submits.
#[cfg(windows)]
const SAVE_DISPATCH_SYSTEM_RVA: usize = er_game_base::rva::SAVE_DISPATCH_SYSTEM_RVA;
/// `FUN_14067dc00` -- the character serializer. Its return value is the SOLE gate on the
/// submit call in both character lanes.
#[cfg(windows)]
const SAVE_SERIALIZE_CHAR_RVA: usize = 0x67dc00;
/// `_DAT_143d69920` -- bytes the character serializer produced on its last call.
///
/// Written exactly ONCE per call, at the merge point after the sub-serializer cascade:
///
/// ```text
///   14067e0aa  CALL 0x141ede890        ; RAX = stream->capacity   (`return *(u64*)(this+0x18)`)
///   14067e0af  MOV  RDI,RAX
///   14067e0b7  CALL 0x141ede7d0        ; RAX = GetBytesLeft()
///   14067e0bc  SUB  RDI,RAX            ; RDI = bytes written
///   14067e0c6  MOV  qword ptr [0x143d69920],RDI
/// ```
///
/// So the stored value is the stream position at the moment the cascade stopped. The only
/// exit that skips the store is the first gate (`0x14067dc30`/`dc3d`/`dc4a`/`dc58` all jump
/// to `0x14067e12f: XOR AL,AL`), and that gate is proven unreachable here: `buf` is
/// null-checked by both calling lanes, `GameMan+0xcb1`/`+0xcb2` are zeroed by
/// `CS::GameMan::GameMan` and have no other writer in the whole image, and `[0x143d68078]`
/// (`GLOBAL_CSEventState`) is a process-lifetime singleton allocated once at save-subsystem
/// boot. Every exit this instrument can observe therefore leaves a fresh count.
///
/// The 12 identical `CALL 0x141ede890; CALL 0x141ede7d0` pairs sprinkled between the steps
/// discard both results (no `MOV`/`SUB` follows them) -- they are vestigial, not a per-step
/// counter, so there is no finer-grained global to read.
#[cfg(windows)]
const SAVE_SERIALIZE_BYTES_RVA: usize = 0x3d69920;

const SAVE_DISPATCH_COMBINED_SIG: &[u8] = &[
    0x48, 0x8B, 0xC4, 0x44, 0x88, 0x40, 0x18, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
    0x48, 0x81, 0xEC, 0xB0, 0x00, 0x00, 0x00,
];
const SAVE_DISPATCH_CHAR_SIG: &[u8] = &[
    0x48, 0x89, 0x5C, 0x24, 0x20, 0x57, 0x41, 0x54, 0x41, 0x57, 0x48, 0x83, 0xEC, 0x30, 0x45, 0x0F,
    0xB6, 0xF8, 0x44, 0x0F, 0xB6, 0xE2, 0x8B, 0xF9,
];
const SAVE_DISPATCH_SYSTEM_SIG: &[u8] = &[
    0x48, 0x8B, 0xC4, 0x57, 0x48, 0x81, 0xEC, 0xA0, 0x00, 0x00, 0x00, 0x48, 0xC7, 0x44, 0x24, 0x20,
    0xFE, 0xFF, 0xFF, 0xFF, 0x48, 0x89, 0x58, 0x08,
];
const SAVE_SERIALIZE_CHAR_SIG: &[u8] = &[
    0x40, 0x55, 0x53, 0x56, 0x57, 0x48, 0x8D, 0x6C, 0x24, 0xA8, 0x48, 0x81, 0xEC, 0x58, 0x01, 0x00,
    0x00, 0x48, 0xC7, 0x45, 0xA0, 0xFE, 0xFF, 0xFF,
];
/// Lane codes for [`dispatch_last_lane`]. Not an enum: it crosses an atomic and lands in
/// telemetry JSON as a number.
pub const SAVE_LANE_NONE: usize = 0;
/// `FUN_14067b750`, the character-slot-only lane.
pub const SAVE_LANE_CHAR: usize = 1;
/// `FUN_14067b570`, the system-slot-only lane.
pub const SAVE_LANE_SYSTEM: usize = 2;
/// `FUN_14067b940`, the combined lane the Save Game commit fires.
pub const SAVE_LANE_COMBINED: usize = 3;

/// [`serialize_last_fail_bytes`] when the byte counter could not be READ -- its address
/// never resolved, or the read faulted. It is NOT a game-state outcome.
///
/// This used to mean "the counter did not move across the failing call, so the serializer
/// was rejected by its first gate". That reading was wrong twice over. The gate is
/// unreachable (see `SAVE_SERIALIZE_BYTES_RVA`), so it was a predicted-impossible outcome;
/// and the before/after comparison that produced it fired on the ordinary case instead --
/// a serializer that aborts at the SAME step every frame stores the SAME count every
/// frame, so "unmoved" was the reading for nearly every real failure. The counter is now
/// read once, after the call, and decoded by [`serialize_fail_step_label`].
pub const SAVE_SERIALIZE_BYTES_UNREADABLE: u64 = u64::MAX;

/// Short, telemetry-facing labels for [`serialize_fail_step_label`]. Kept as constants so
/// a harness can match on them and a test can assert them without restating a literal.
pub const SAVE_SERIALIZE_STEP_UNREADABLE: &str = "byte-counter-unreadable";
/// Step 1 -- the 0x10-byte header write produced nothing.
pub const SAVE_SERIALIZE_STEP_HEADER_NONE: &str = "step1-header-write-nothing";
/// Step 1 -- the 0x10-byte header write was short.
pub const SAVE_SERIALIZE_STEP_HEADER_SHORT: &str = "step1-header-write-short";
/// Step 2 -- the 0x10-byte xorshift-seed write produced nothing.
pub const SAVE_SERIALIZE_STEP_RANDSEED_NONE: &str = "step2-randseed-write-nothing";
/// Step 2 -- the 0x10-byte xorshift-seed write was short.
pub const SAVE_SERIALIZE_STEP_RANDSEED_SHORT: &str = "step2-randseed-write-short";
/// Step 3 -- `FUN_140257f20` (the GameDataMan chain) refused having written nothing.
pub const SAVE_SERIALIZE_STEP_GAMEDATAMAN_NONE: &str = "step3-gamedataman-no-output";
/// Step 3 or later -- output was produced past the last statically fixed boundary.
pub const SAVE_SERIALIZE_STEP_AFTER_OUTPUT: &str = "step3plus-after-output";

/// Decode `_DAT_143d69920` into the name of the `FUN_14067dc00` step that refused.
///
/// The serializer is a straight-line cascade -- each step runs only if the previous one
/// returned true -- and the byte counter is stored once, at the merge point, so the count
/// IS the stream position where the cascade stopped. Cumulative boundaries therefore name
/// the failing step, but only as far as the steps have statically fixed sizes:
///
/// | bytes | exit |
/// |-------|------|
/// | `0x00` | step 1 `Write(header,0x10)` @`0x14067dd3d` wrote nothing |
/// | `0x01..=0x0f` | step 1 short write |
/// | `0x10` | step 2 `Write(4 x CS::CSRandXorshift::NextInt, 0x10)` @`0x14067ddcc` wrote nothing |
/// | `0x11..=0x1f` | step 2 short write |
/// | `0x20` | step 3 `FUN_140257f20` refused with no output |
/// | `> 0x20` | inside step 3 after output, or any later step |
///
/// Only steps 1 and 2 are fixed-size (0x10 each), which is why the map stops at `0x20`.
/// Step 3 (`FUN_140257f20`) opens with `PlayerGameData::Serialize` and then writes an
/// optional 0x40-byte bloodstain block plus a 4-byte entity id, so its length is decided by
/// game state and no later cumulative boundary is a constant. Separating the twelve steps
/// past that point needs a hook per sub-serializer, not more arithmetic on this count.
///
/// `0x20` is the interesting one: `FUN_140257f20`'s first act is
/// `if (GLOBAL_GameDataMan->mainPlayerGameData == 0) return false`, so a count of exactly
/// `0x20` is the signature of "the character has no serializable player data".
pub fn serialize_fail_step_label(bytes: u64) -> &'static str {
    match bytes {
        SAVE_SERIALIZE_BYTES_UNREADABLE => SAVE_SERIALIZE_STEP_UNREADABLE,
        0 => SAVE_SERIALIZE_STEP_HEADER_NONE,
        0x01..=0x0f => SAVE_SERIALIZE_STEP_HEADER_SHORT,
        0x10 => SAVE_SERIALIZE_STEP_RANDSEED_NONE,
        0x11..=0x1f => SAVE_SERIALIZE_STEP_RANDSEED_SHORT,
        0x20 => SAVE_SERIALIZE_STEP_GAMEDATAMAN_NONE,
        _ => SAVE_SERIALIZE_STEP_AFTER_OUTPUT,
    }
}

/// The long-form reading of [`serialize_fail_step_label`], for a log line that has to be
/// actionable on its own. Same input, same partition -- this one names the native function
/// and says what to do next.
pub fn serialize_fail_step_detail(bytes: u64) -> &'static str {
    match bytes {
        SAVE_SERIALIZE_BYTES_UNREADABLE => {
            "the byte counter 0x143d69920 could not be read, so the failing step is unknown; \
             the counter address never resolved"
        }
        0 => {
            "step 1 of FUN_14067dc00 -- the 0x10-byte header Write returned short having \
             written nothing, so the 0x280000 output stream refused its very first write"
        }
        0x01..=0x0f => {
            "step 1 of FUN_14067dc00 -- the 0x10-byte header Write was cut short mid-write"
        }
        0x10 => {
            "step 2 of FUN_14067dc00 -- the 0x10-byte xorshift-seed Write returned short \
             having written nothing; the header went out but the stream then refused"
        }
        0x11..=0x1f => {
            "step 2 of FUN_14067dc00 -- the 0x10-byte xorshift-seed Write was cut short \
             mid-write"
        }
        0x20 => {
            "step 3 of FUN_14067dc00 -- FUN_140257f20, the GameDataMan chain, refused having \
             written NOTHING. Its first act is `if (GLOBAL_GameDataMan->mainPlayerGameData \
             == 0) return false`, so this count is the signature of a character with no \
             serializable player data"
        }
        _ => {
            "past the last statically fixed boundary of FUN_14067dc00: the abort is inside \
             FUN_140257f20 after it produced output, or in one of the later steps \
             (FUN_1405b5c60 event flags, FUN_14067e290, FUN_14067eaa0, FUN_14067e9a0, \
             FUN_1401cd510 CSNetMan, FUN_140647420, FUN_140643560, FUN_140258640, \
             FUN_1402586f0, FUN_1402585e0, FUN_1402586a0, FUN_14067c590 + the 0x80-byte \
             tail). Step 3 is variable-length, so no later boundary is a constant and this \
             count cannot separate them -- naming one needs a hook per sub-serializer"
        }
    }
}

#[cfg(windows)]
type EnqueueSaveJobFn = unsafe extern "system" fn(usize, u32) -> u8;
#[cfg(windows)]
type PollSaveStatusFn = unsafe extern "system" fn(usize) -> u32;
#[cfg(windows)]
type ReleaseRequestFn = unsafe extern "system" fn(usize);
#[cfg(windows)]
static ORIG_ENQUEUE_SAVE_JOB: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_POLL_SAVE_STATUS: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static SL_RELEASE_REQUEST: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
static ORIG_SAVE_DISPATCH_COMBINED: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SAVE_DISPATCH_CHAR: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SAVE_DISPATCH_SYSTEM: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_SAVE_SERIALIZE_CHAR: AtomicUsize = AtomicUsize::new(0);

/// The two detours that actually suppress: the submit swallow and the status rewrite.
/// The quit-settle observer is deliberately NOT one of them -- it changes nothing and a
/// failure to install it must not read as a partial suppression.
pub const SUPPRESSOR_HOOKS: usize = 2;

static ARMED: AtomicUsize = AtomicUsize::new(0);
static INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SETTLE_OBSERVER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static PROLOGUE_MISMATCHES: AtomicUsize = AtomicUsize::new(0);

static SUBMITS_SWALLOWED: AtomicU64 = AtomicU64::new(0);
static SUBMITS_PASSED_THROUGH: AtomicU64 = AtomicU64::new(0);
static STATUS_FAKED: AtomicU64 = AtomicU64::new(0);
/// Of [`STATUS_FAKED`], the rewrites issued while `GameMan.saveState == 0` -- i.e. the game
/// had NO save in flight, so the rewrite retired nothing. See [`status_faked_idle`].
static STATUS_FAKED_IDLE: AtomicU64 = AtomicU64::new(0);
static STATUS_PASSED_THROUGH: AtomicU64 = AtomicU64::new(0);
static RELEASE_UNAVAILABLE: AtomicU64 = AtomicU64::new(0);

// ---- save-dispatch observers (see the block comment on the RVAs above) ----
static DISPATCH_CALLS: AtomicU64 = AtomicU64::new(0);
static DISPATCH_DECLINES: AtomicU64 = AtomicU64::new(0);
static DISPATCH_LAST_LANE: AtomicUsize = AtomicUsize::new(SAVE_LANE_NONE);
/// Declines observed while a one-shot bypass token was pending. Non-zero is the decisive
/// answer to "did the dispatcher never run, or did it run and refuse?" -- it ran and
/// refused, and no amount of waiting at the enqueue will change that.
static DISPATCH_DECLINES_WITH_BYPASS: AtomicU64 = AtomicU64::new(0);
static SERIALIZE_CALLS: AtomicU64 = AtomicU64::new(0);
static SERIALIZE_FAILURES: AtomicU64 = AtomicU64::new(0);
static SERIALIZE_LAST_FAIL_BYTES: AtomicU64 = AtomicU64::new(SAVE_SERIALIZE_BYTES_UNREADABLE);
static DISPATCH_OBSERVERS_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Resolved address of the serializer's byte counter (0 = unresolved).
#[cfg(windows)]
static SERIALIZE_BYTES_ADDR: AtomicUsize = AtomicUsize::new(0);
/// One "the dispatcher refused the user's save" line per arm, not per frame: the lane is
/// re-entered every frame while the request stays latched.
static BYPASS_DECLINE_REPORTED: AtomicUsize = AtomicUsize::new(0);

// ============================================================================
// ONE-SHOT BYPASS (save-game-flow WP1). A single-use token that lets exactly one SL
// save enqueue through to the real trampoline. Armed by the product's Save Game commit
// path immediately before it fires the forced native request pair; consumed by the
// FIRST enqueue that arrives afterwards; expired by the product's watchdog if that
// enqueue never comes, so a stranded token can never leak onto some later native save.
// ============================================================================

/// 0 = no token, 1 = armed. CAS-only transitions.
static BYPASS_TOKEN: AtomicUsize = AtomicUsize::new(0);
/// Set when the token is consumed; tells the status-poll detour to latch the first
/// terminal (non-in-flight) status of the real, bypassed save.
static BYPASS_COMPLETION_WATCH: AtomicUsize = AtomicUsize::new(0);
static BYPASS_ARMED_TOTAL: AtomicU64 = AtomicU64::new(0);
static BYPASS_ALLOWED_TOTAL: AtomicU64 = AtomicU64::new(0);
static BYPASS_ALLOWED_FAILED_TOTAL: AtomicU64 = AtomicU64::new(0);
static BYPASS_EXPIRED_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Latched terminal status of the last bypassed save (0 = success; see the poll's
/// jump-table codes). Holds [`BYPASS_FINAL_STATUS_NONE`] until the first capture and is
/// re-sentineled on every arm, so telemetry always shows the CURRENT commit's outcome.
static BYPASS_FINAL_STATUS: AtomicU32 = AtomicU32::new(BYPASS_FINAL_STATUS_NONE);
/// Handshake flag: set with each fresh [`BYPASS_FINAL_STATUS`] capture, consumed by
/// [`take_bypass_final_status`] so the caller's state machine sees each outcome once
/// while the latched value itself stays readable for telemetry.
static BYPASS_FINAL_STATUS_FRESH: AtomicUsize = AtomicUsize::new(0);

/// Sentinel for "no terminal status captured yet".
pub const BYPASS_FINAL_STATUS_NONE: u32 = 0xffff_ffff;

/// Arm the one-shot bypass: the NEXT SL save enqueue is forwarded for real instead of
/// swallowed. Returns false (and arms nothing) when suppression is not armed -- with no
/// swallow in place every save already writes, so a token would be meaningless -- or
/// when a token is already pending. Logged and published unconditionally: arming is a
/// rare, user-initiated event.
pub fn arm_one_save_bypass() -> bool {
    if !is_armed() {
        log_message(format_args!(
            "suppress: bypass arm REFUSED -- suppression is not armed, saves already write natively"
        ));
        return false;
    }
    match BYPASS_TOKEN.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => {
            BYPASS_FINAL_STATUS.store(BYPASS_FINAL_STATUS_NONE, Ordering::SeqCst);
            BYPASS_FINAL_STATUS_SOURCE.store(BYPASS_STATUS_SOURCE_NONE, Ordering::SeqCst);
            BYPASS_FINAL_STATUS_FRESH.store(0, Ordering::SeqCst);
            BYPASS_COMPLETION_WATCH.store(0, Ordering::SeqCst);
            BYPASS_DECLINE_REPORTED.store(0, Ordering::SeqCst);
            let count = BYPASS_ARMED_TOTAL.fetch_add(1, Ordering::SeqCst) + 1;
            log_message(format_args!(
                "suppress: one-shot bypass ARMED (#{count}) -- the next SL save enqueue will be forwarded for real"
            ));
            publish_snapshot();
            true
        }
        Err(_) => {
            log_message(format_args!(
                "suppress: bypass arm REFUSED -- a token is already pending"
            ));
            false
        }
    }
}

/// True while an armed token has not yet been consumed by an enqueue.
pub fn bypass_pending() -> bool {
    BYPASS_TOKEN.load(Ordering::SeqCst) != 0
}

/// Expire a still-pending token (watchdog path). True if a token was actually revoked.
/// This is a FAILURE: the user's explicit save request never produced an enqueue, so it
/// is logged and published unconditionally (noise rule 3).
pub fn expire_bypass_if_pending() -> bool {
    if BYPASS_TOKEN
        .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        BYPASS_COMPLETION_WATCH.store(0, Ordering::SeqCst);
        let count = BYPASS_EXPIRED_TOTAL.fetch_add(1, Ordering::SeqCst) + 1;
        log_message(format_args!(
            "suppress: one-shot bypass EXPIRED unconsumed (#{count}) -- no SL save enqueue arrived; the user's save did NOT happen"
        ));
        publish_snapshot();
        true
    } else {
        false
    }
}

/// Peek at the freshness handshake without consuming it.
///
/// A caller that must do something fallible BEFORE it may consume the outcome -- the save flow
/// has to score the destination file first, and that scoring can be deferred while the native
/// writer is still inside a job body -- needs to know a status is waiting without taking it. A
/// consumed status that is then dropped on a deferral would be an outcome nobody ever reports.
pub fn bypass_final_status_fresh() -> bool {
    BYPASS_FINAL_STATUS_FRESH.load(Ordering::SeqCst) != 0
}

/// Consume the freshly-captured terminal status of the last bypassed save, if one has
/// been captured since the last arm. The latched value itself is NOT cleared (telemetry
/// keeps reporting it); only the freshness handshake is consumed, so a state machine
/// polling this sees each outcome exactly once.
pub fn take_bypass_final_status() -> Option<u32> {
    if BYPASS_FINAL_STATUS_FRESH.swap(0, Ordering::SeqCst) != 0 {
        Some(BYPASS_FINAL_STATUS.load(Ordering::SeqCst))
    } else {
        None
    }
}

/// The latched terminal status of the last bypassed save, or
/// [`BYPASS_FINAL_STATUS_NONE`] when none has been captured. Telemetry accessor.
pub fn bypass_final_status_raw() -> u32 {
    BYPASS_FINAL_STATUS.load(Ordering::SeqCst)
}

/// Bypass counters as (name, value) pairs, for hosts that serialize by iteration.
pub fn bypass_counters() -> [(&'static str, u64); 5] {
    [
        (
            "save_bypass_armed_total",
            BYPASS_ARMED_TOTAL.load(Ordering::SeqCst),
        ),
        (
            "save_bypass_allowed_total",
            BYPASS_ALLOWED_TOTAL.load(Ordering::SeqCst),
        ),
        (
            "save_bypass_allowed_failed_total",
            BYPASS_ALLOWED_FAILED_TOTAL.load(Ordering::SeqCst),
        ),
        (
            "save_bypass_expired_total",
            BYPASS_EXPIRED_TOTAL.load(Ordering::SeqCst),
        ),
        (
            "save_bypass_final_status",
            u64::from(BYPASS_FINAL_STATUS.load(Ordering::SeqCst)),
        ),
    ]
}

/// Times a bypass token was armed.
pub fn bypass_armed_total() -> u64 {
    BYPASS_ARMED_TOTAL.load(Ordering::SeqCst)
}

/// Times a token was consumed and the enqueue forwarded for real.
pub fn bypass_allowed_total() -> u64 {
    BYPASS_ALLOWED_TOTAL.load(Ordering::SeqCst)
}

/// Times a forwarded enqueue could not be submitted (trampoline unset or the native
/// enqueue itself returned failure).
pub fn bypass_allowed_failed_total() -> u64 {
    BYPASS_ALLOWED_FAILED_TOTAL.load(Ordering::SeqCst)
}

/// Times a pending token was expired unconsumed by the watchdog.
pub fn bypass_expired_total() -> u64 {
    BYPASS_EXPIRED_TOTAL.load(Ordering::SeqCst)
}

/// `GameMan+0xbc4 == 2`: the return-to-title save was submitted and the wait job is
/// still spinning. This is the ONLY state from which `FUN_14067a980` does anything.
#[cfg(windows)]
const QUIT_PHASE_SAVE_SUBMITTED: usize = 2;
/// Highest return-to-title phase ever observed. A secondary diagnostic only: it says
/// how FAR the quit got (1 = requested, 2 = save submitted), which is useful for
/// locating a hang, but it cannot certify success -- see `QUIT_PHASE_SETTLE_EVENTS`.
static QUIT_PHASE_MAX_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Times the 2 -> 3 transition actually executed, counted as an EVENT at the only
/// function that performs it.
///
/// Sampling the field could never prove this. `bc4 == 3` is TRANSIENT: `FUN_14067a980`
/// sets it, the quit chain's wait job consumes it, and `FUN_14067a970(0)` resets it to
/// 0. Two runs with a user-confirmed working quit both ended with the sampled maximum
/// at 2, because the value simply never existed at a moment anything sampled it.
static QUIT_PHASE_SETTLE_EVENTS: AtomicU64 = AtomicU64::new(0);
#[cfg(windows)]
static ORIG_QUIT_PHASE_SETTLE: AtomicUsize = AtomicUsize::new(0);

/// Number of detours actually bound (0 or 2).
pub fn installed_hooks() -> usize {
    INSTALLED.load(Ordering::SeqCst)
}

/// True once both suppressor detours are bound and swallowing is active.
pub fn is_armed() -> bool {
    ARMED.load(Ordering::SeqCst) != 0
}

/// Whether the quit-settle observer bound. When false, `quit_phase_settle_events` can
/// only ever be 0, and a harness must not read that as a deadlock.
pub fn settle_observer_installed() -> bool {
    SETTLE_OBSERVER_INSTALLED.load(Ordering::SeqCst) != 0
}

/// Suppression counters as (name, value) pairs, for hosts that serialize by iteration.
pub fn counters() -> [(&'static str, u64); 36] {
    [
        (
            "suppress_submits_swallowed",
            SUBMITS_SWALLOWED.load(Ordering::SeqCst),
        ),
        (
            "suppress_submits_passed_through",
            SUBMITS_PASSED_THROUGH.load(Ordering::SeqCst),
        ),
        ("suppress_status_faked", STATUS_FAKED.load(Ordering::SeqCst)),
        (
            "suppress_status_faked_idle",
            STATUS_FAKED_IDLE.load(Ordering::SeqCst),
        ),
        ("save_dispatch_calls", DISPATCH_CALLS.load(Ordering::SeqCst)),
        (
            "save_dispatch_declines",
            DISPATCH_DECLINES.load(Ordering::SeqCst),
        ),
        (
            "save_dispatch_declines_with_bypass",
            DISPATCH_DECLINES_WITH_BYPASS.load(Ordering::SeqCst),
        ),
        (
            "save_dispatch_last_lane",
            DISPATCH_LAST_LANE.load(Ordering::SeqCst) as u64,
        ),
        (
            "save_serialize_calls",
            SERIALIZE_CALLS.load(Ordering::SeqCst),
        ),
        (
            "save_serialize_failures",
            SERIALIZE_FAILURES.load(Ordering::SeqCst),
        ),
        (
            "save_serialize_last_fail_bytes",
            SERIALIZE_LAST_FAIL_BYTES.load(Ordering::SeqCst),
        ),
        (
            "suppress_status_passed_through",
            STATUS_PASSED_THROUGH.load(Ordering::SeqCst),
        ),
        (
            "suppress_release_unavailable",
            RELEASE_UNAVAILABLE.load(Ordering::SeqCst),
        ),
        (
            "suppress_prologue_mismatches",
            PROLOGUE_MISMATCHES.load(Ordering::SeqCst) as u64,
        ),
        // How far the quit got. NOT a success oracle: a healthy quit ends here at 2,
        // because 3 is transient. Read it only to locate a hang.
        (
            "quit_phase_bc4_max_seen",
            QUIT_PHASE_MAX_SEEN.load(Ordering::SeqCst) as u64,
        ),
        // The deadlock oracle: non-zero means the quit-to-title wait job was released.
        // Unlike the sampled maximum, an event cannot be missed.
        (
            "quit_phase_settle_events",
            QUIT_PHASE_SETTLE_EVENTS.load(Ordering::SeqCst),
        ),
        // SL REQUEST SLOT. `swallow_release_left_dirty` is the self-incrimination oracle:
        // non-zero means our own swallow left the submit precondition failing and every
        // later save in the process is refused because of us.
        (
            "save_swallow_release_left_dirty",
            SWALLOW_RELEASE_LEFT_DIRTY.load(Ordering::SeqCst),
        ),
        (
            "save_swallow_iodev_mismatch",
            SWALLOW_IODEV_MISMATCH.load(Ordering::SeqCst),
        ),
        (
            "save_iodev_slot_read_failures",
            SLOT_READ_FAILURES.load(Ordering::SeqCst),
        ),
        (
            "save_dispatch_last_decline_bail_reason",
            DECLINE_BAIL_REASON.load(Ordering::SeqCst) as u64,
        ),
        // THE LOAD CONSUMER. `load_consumer_stranded` is the regression oracle for the
        // post-switch save refusal: non-zero means a completed load kept the shared
        // `iodev+0x20` job and no save can be built again. `load_consumer_releases` is its
        // positive counterpart -- proof the slot was freed, by the native consumer, at the
        // substitution site. `load_consumer_still_held` counts the times the native guard
        // refused to take a load that had not finished, which is what makes the release
        // incapable of racing one.
        (
            "save_load_consumer_calls",
            LOAD_CONSUMER_CALLS.load(Ordering::SeqCst),
        ),
        (
            "save_load_consumer_releases",
            LOAD_CONSUMER_RELEASES.load(Ordering::SeqCst),
        ),
        (
            "save_load_consumer_still_held",
            LOAD_CONSUMER_STILL_HELD_COUNT.load(Ordering::SeqCst),
        ),
        (
            "save_load_consumer_stranded",
            LOAD_CONSUMER_STRANDED.load(Ordering::SeqCst),
        ),
        (
            "save_load_consumer_last_outcome",
            LOAD_CONSUMER_LAST_OUTCOME.load(Ordering::SeqCst) as u64,
        ),
        // THE WRITE-COMPLETION EVENT. `save_job_starts`/`save_job_completions` are the SL
        // worker actually picking up and finishing a save; `save_job_last_result` is the
        // game's own verdict on it (0 = success). `save_job_observer_installed == 0` means
        // none of the three can be trusted as absence-of-write, because nothing is watching.
        (
            "save_job_observer_installed",
            SAVE_JOB_OBSERVER_INSTALLED.load(Ordering::SeqCst) as u64,
        ),
        ("save_job_starts", SAVE_JOB_STARTS.load(Ordering::SeqCst)),
        (
            "save_job_completions",
            SAVE_JOB_COMPLETIONS.load(Ordering::SeqCst),
        ),
        (
            "save_job_last_result",
            u64::from(SAVE_JOB_LAST_RESULT.load(Ordering::SeqCst)),
        ),
        (
            "save_job_no_trampoline",
            SAVE_JOB_NO_TRAMPOLINE.load(Ordering::SeqCst),
        ),
        // WHICH observation completed each commit. Their sum should equal the number of
        // successful commits; a shortfall is commits that only the watchdog ended.
        (
            "save_bypass_completed_via_job",
            BYPASS_COMPLETED_VIA_JOB_TOTAL.load(Ordering::SeqCst),
        ),
        (
            "save_bypass_completed_via_poll",
            BYPASS_COMPLETED_VIA_POLL_TOTAL.load(Ordering::SeqCst),
        ),
        // WHICH WRITE BRANCH RAN. Read the installed count FIRST: at 0 the two call counters
        // can only be 0 and that is "nothing was watching", not "nothing was written". At 2,
        // both reading 0 means no save was written at all during the run.
        (
            "save_write_branch_observers_installed",
            WRITE_BRANCH_OBSERVERS_INSTALLED.load(Ordering::SeqCst) as u64,
        ),
        (
            "save_write_full_rebuild_calls",
            WRITE_FULL_REBUILD_CALLS.load(Ordering::SeqCst),
        ),
        (
            "save_write_in_place_calls",
            WRITE_IN_PLACE_CALLS.load(Ordering::SeqCst),
        ),
        (
            "save_write_branch_no_trampoline",
            WRITE_BRANCH_NO_TRAMPOLINE.load(Ordering::SeqCst),
        ),
    ]
}

/// Named accessors for the counters the product telemetry exports individually.
pub fn submits_swallowed() -> u64 {
    SUBMITS_SWALLOWED.load(Ordering::SeqCst)
}

/// Submits that reached the real enqueue while suppression was DISARMED (positive
/// control) -- distinct from bypass allows, which happen while armed.
pub fn submits_passed_through() -> u64 {
    SUBMITS_PASSED_THROUGH.load(Ordering::SeqCst)
}

/// Polls whose "no request" answer was rewritten to success.
pub fn status_faked() -> u64 {
    STATUS_FAKED.load(Ordering::SeqCst)
}

/// Of [`status_faked`], the rewrites that answered a poll made while the game had NO save
/// in flight (`GameMan.saveState == 0`).
///
/// Read the pair, never `status_faked` alone. The rewrite is what retires a SWALLOWED save
/// (`saveState == 1`), and only those rewrites did anything; an idle rewrite is a no-op for
/// every consumer that matters, because `FUN_140679510`/`FUN_1406794b0` treat 4 and 0
/// identically (both are "not 1") and the only callers that distinguish them --
/// `DoSaveStuff` and the "saving..." MenuJob `FUN_14082a0f0` -- are not running when there
/// is nothing to retire. A large `status_faked` with `status_faked_idle` almost equal to it
/// therefore says the suppressor did NOTHING that run, which is exactly the reading a bare
/// `status_faked = 207` against 2 swallows failed to give.
///
/// The lie itself is deliberately NOT narrowed to an outstanding swallow. Narrowing it
/// risks answering a poll with the raw 4, and 4 is catastrophic in the other direction:
/// `FUN_14082a0f0` maps it to `MenuJobResult::Failed` and `DoSaveStuff` maps it to a silent
/// no-op that never calls `FUN_14067a980`, so `GameMan+0xbc4` never reaches 3 and
/// System->Quit spins forever. Two different pollers can run in the same frame, so any
/// consume-once scheme under-lies on the second one. Over-lying is inert; under-lying
/// deadlocks the quit.
pub fn status_faked_idle() -> u64 {
    STATUS_FAKED_IDLE.load(Ordering::SeqCst)
}

/// Entries into any of the three native save-dispatch lanes.
pub fn dispatch_calls() -> u64 {
    DISPATCH_CALLS.load(Ordering::SeqCst)
}

/// Dispatch-lane entries that returned 0 -- the lane refused and touched nothing, so the
/// request flags stay latched and no SL enqueue is created.
pub fn dispatch_declines() -> u64 {
    DISPATCH_DECLINES.load(Ordering::SeqCst)
}

/// Dispatch declines observed while a one-shot bypass token was pending. Non-zero means
/// the user's Save Game request DID reach the native dispatcher and the dispatcher refused
/// it -- the failure is upstream of the enqueue and upstream of this crate.
pub fn dispatch_declines_with_bypass() -> u64 {
    DISPATCH_DECLINES_WITH_BYPASS.load(Ordering::SeqCst)
}

/// Lane of the most recent dispatch entry (`SAVE_LANE_*`).
pub fn dispatch_last_lane() -> usize {
    DISPATCH_LAST_LANE.load(Ordering::SeqCst)
}

/// Entries into the character serializer `FUN_14067dc00`.
pub fn serialize_calls() -> u64 {
    SERIALIZE_CALLS.load(Ordering::SeqCst)
}

/// Character-serializer calls that returned 0. Each one is a character save that produced
/// no submit at all.
pub fn serialize_failures() -> u64 {
    SERIALIZE_FAILURES.load(Ordering::SeqCst)
}

/// Bytes the character serializer had produced when it last failed -- `_DAT_143d69920` read
/// straight after the failing call -- or [`SAVE_SERIALIZE_BYTES_UNREADABLE`] when the
/// counter could not be read at all.
///
/// Raw on purpose; [`serialize_last_fail_step`] turns it into a step name. Note the count
/// repeats across frames: the serializer aborts at the same step on every re-entry while
/// the request stays latched, so an unchanging value is the expected shape of a stuck save,
/// not a sign the instrument stopped working.
pub fn serialize_last_fail_bytes() -> u64 {
    SERIALIZE_LAST_FAIL_BYTES.load(Ordering::SeqCst)
}

/// The `FUN_14067dc00` step that refused on the last failing call, decoded from
/// [`serialize_last_fail_bytes`] by [`serialize_fail_step_label`].
pub fn serialize_last_fail_step() -> &'static str {
    serialize_fail_step_label(SERIALIZE_LAST_FAIL_BYTES.load(Ordering::SeqCst))
}

/// Number of dispatch/serializer observers bound (0..=4). Zero means the attribution
/// counters above can only ever read 0 and a harness must NOT read that as "no dispatch".
pub fn dispatch_observers_installed() -> usize {
    DISPATCH_OBSERVERS_INSTALLED.load(Ordering::SeqCst)
}

/// Install-time prologue verification failures (nonzero = wrong game build).
pub fn prologue_mismatches() -> u64 {
    PROLOGUE_MISMATCHES.load(Ordering::SeqCst) as u64
}

/// Quit-to-title settle events observed (the bc4 2 -> 3 transition).
pub fn settle_events() -> u64 {
    QUIT_PHASE_SETTLE_EVENTS.load(Ordering::SeqCst)
}

/// Swallows that ran with no resolved `FUN_140e6f200` address and were therefore passed
/// through to the real enqueue -- each one is a save that was WRITTEN.
///
/// `install` refuses to arm without that address, so this should be structurally
/// unreachable; it is exported because "unreachable" is a claim about code that has never
/// been measured, and the failure it would represent (a real write plus a leaked request)
/// is the exact shape of the bug this instrument exists to find.
pub fn release_unavailable() -> u64 {
    RELEASE_UNAVAILABLE.load(Ordering::SeqCst)
}

/// Swallows whose `FUN_140e6f200` release did NOT restore the submit builders'
/// `iodev+0x10 == 0 && iodev+0x20 == 0` precondition.
///
/// This is the direct test of "does our swallow leave the request slot dirty". Zero with
/// a non-zero swallow count DISPROVES it; any non-zero value proves it, and
/// [`swallow_slot_after`] names the field that stayed populated.
pub fn swallow_release_left_dirty() -> u64 {
    SWALLOW_RELEASE_LEFT_DIRTY.load(Ordering::SeqCst)
}

/// Swallows where the `iodev` argument differed from the `DAT_144589390` singleton, i.e.
/// the release would have been applied to a different object than the one the builders
/// check. Structurally impossible (every caller reaches the device through
/// `FUN_140e6e060`), and measured anyway.
pub fn swallow_iodev_mismatch() -> u64 {
    SWALLOW_IODEV_MISMATCH.load(Ordering::SeqCst)
}

/// Slot samples that could not be read. Non-zero means the `iodev_*` oracles are stale and
/// a decline classified as `iodev-unreadable` proves nothing either way.
pub fn slot_read_failures() -> u64 {
    SLOT_READ_FAILURES.load(Ordering::SeqCst)
}

/// The SL request slot as it stood at the most recent dispatch decline, or `None` if no
/// decline has been sampled (or the sample failed).
pub fn decline_slot() -> Option<SlRequestSlot> {
    DECLINE_SLOT.snapshot()
}

/// The SL request slot immediately BEFORE the last swallow's release call.
pub fn swallow_slot_before() -> Option<SlRequestSlot> {
    SWALLOW_SLOT_BEFORE.snapshot()
}

/// The SL request slot immediately AFTER the last swallow's release call. Every field
/// should be zero; `save_content` or `job` non-zero is a self-inflicted stuck slot.
pub fn swallow_slot_after() -> Option<SlRequestSlot> {
    SWALLOW_SLOT_AFTER.snapshot()
}

/// Reason code (`SL_BAIL_*`) for the most recent dispatch decline. Pair it with
/// [`sl_bail_reason_label`] for the name.
pub fn decline_bail_reason() -> usize {
    DECLINE_BAIL_REASON.load(Ordering::SeqCst)
}

/// Name of the most recent dispatch decline's bail reason.
pub fn decline_bail_reason_label() -> &'static str {
    sl_bail_reason_label(DECLINE_BAIL_REASON.load(Ordering::SeqCst))
}

/// Decide what a poll should report.
///
/// Split out as a pure function so the one rule that matters -- *only ever rewrite the
/// "no request" code, and only after we have actually swallowed something* -- is unit
/// testable on the host, with no game and no hooking involved.
fn decide_status(raw: u32, armed: bool, swallowed: u64) -> u32 {
    if armed && swallowed > 0 && raw == SL_STATUS_NO_REQUEST {
        SL_STATUS_SUCCESS
    } else {
        raw
    }
}

/// True when `actual` starts with `expected`.
///
/// Kept separate from the hooking code for the same reason: an address guard that is
/// itself unverified would be decoration.
fn prologue_matches(actual: &[u8], expected: &[u8]) -> bool {
    actual.len() >= expected.len() && &actual[..expected.len()] == expected
}

/// Whether occurrence `count` of a repeating event earns a log line.
///
/// The justification is that a repeat carries no information, NOT that saves are
/// enormously frequent. Measured rate is 7-25 swallowed submits per session, so the
/// throttle saves tens of lines, not thousands. (An earlier version of this comment
/// claimed the rune-counter widget drives a save on every rune change and implied
/// thousands per session. The save site is real -- `FUN_1408d4a30` calls
/// `CSMenuManImp::RequestSave(.., 7)` when the rune total changes -- but it is gated by
/// the widget's own state machine at `+0x2a0` and the requests coalesce through the
/// `GameMan+0xb72`/`+0xb73` flags, so the rate is nothing like that.)
///
/// What stands on its own: the 2nd and the 400th line are character-for-character
/// identical apart from the counter, each costs an open/append/close, and the count is
/// already in the JSON where a harness actually reads it.
///
/// The rule keeps the first occurrence, then only exponentially spaced milestones, so N
/// repeats cost O(log N) lines while the magnitude stays visible. `novel` overrides it --
/// a genuinely new *kind* of event is always worth a line however late it shows up.
fn should_report(count: u64, novel: bool) -> bool {
    novel || count.is_power_of_two()
}

/// Opcodes already seen at the choke point, as a bitmask.
///
/// A save opcode never seen before means a different *kind* of save funnelled through,
/// which is exactly the sort of thing this crate exists to discover -- so it is reported
/// however many identical saves preceded it. Bit 63 is a catch-all for opcode >= 63:
/// every opcode observed so far is 0, and a dense high opcode space would otherwise
/// need a wider structure for no benefit.
static SEEN_OPCODES: AtomicU64 = AtomicU64::new(0);

/// Record `opcode` and report whether it had never been seen before.
fn note_opcode(opcode: u32) -> bool {
    let bit = 1_u64 << opcode.min(63);
    SEEN_OPCODES.fetch_or(bit, Ordering::SeqCst) & bit == 0
}

#[cfg(windows)]
fn verify(rva: usize, expected: &[u8], name: &str) -> Option<usize> {
    let address = match game_rva(rva as u32) {
        Ok(address) => address,
        Err(err) => {
            log_message(format_args!("suppress: {name}: cannot resolve RVA: {err}"));
            return None;
        }
    };
    let mut actual = [0_u8; 32];
    let window = &mut actual[..expected.len()];
    if !unsafe { read_bytes(address, window) } {
        log_message(format_args!(
            "suppress: {name} @0x{address:x}: prologue unreadable"
        ));
        PROLOGUE_MISMATCHES.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    if !prologue_matches(window, expected) {
        log_message(format_args!(
            "suppress: {name} @0x{address:x}: prologue mismatch (got {:02x?}, want {:02x?}) \
             -- refusing to hook; this build is not the 1.16.2 image these addresses were \
             verified against",
            window, expected
        ));
        PROLOGUE_MISMATCHES.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    Some(address)
}

/// Install the suppression detours. Returns the number bound.
///
/// `disarm_for_census` is the standalone DLL's positive-control lever: true skips the
/// install entirely (saves write normally so the census can observe them). The env-var
/// consultation that used to live here moved OUT to that caller -- the product passes
/// `false` unconditionally, so no env var can alter product behavior.
///
/// All-or-nothing on purpose. Binding only the submit detour would leave every save
/// stuck reporting "no request" and hang System->Quit on the `bc4 == 3` wait; binding
/// only the status detour would rewrite statuses for saves that really happened. A
/// partial install is worse than none, so a failure of either backs the whole thing out.
#[cfg(windows)]
pub fn install(disarm_for_census: bool) -> usize {
    if disarm_for_census {
        log_message(format_args!(
            "suppress: DISARMED by caller -- census-only positive-control run; saves will \
             be written normally so the census must observe them"
        ));
        return 0;
    }

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!("suppress: MH_Initialize failed: {status:?}"));
            return 0;
        }
    }

    let Some(enqueue) = verify(
        SL_ENQUEUE_SAVE_JOB_RVA,
        SL_ENQUEUE_SAVE_JOB_SIG,
        "SL_EnqueueSaveJob",
    ) else {
        return 0;
    };
    let Some(poll) = verify(
        SL_POLL_SAVE_STATUS_RVA,
        SL_POLL_SAVE_STATUS_SIG,
        "SL_PollSaveStatus",
    ) else {
        return 0;
    };
    let Some(release) = verify(
        SL_RELEASE_REQUEST_RVA,
        SL_RELEASE_REQUEST_SIG,
        "SL_ReleaseRequest",
    ) else {
        return 0;
    };
    SL_RELEASE_REQUEST.store(release, Ordering::SeqCst);

    // The quit-settle observer. Not a suppressor -- it calls the original and only
    // counts. It exists because sampling GameMan+0xbc4 provably cannot see the 2 -> 3
    // transition: the value is consumed and reset within the same quit sequence.
    let settle = verify(
        QUIT_PHASE_SETTLE_RVA,
        QUIT_PHASE_SETTLE_SIG,
        "QuitPhaseSettle",
    );

    let targets: [(&str, usize, *mut c_void, &AtomicUsize); 2] = [
        (
            "SL_EnqueueSaveJob",
            enqueue,
            enqueue_save_job_hook as *mut c_void,
            &ORIG_ENQUEUE_SAVE_JOB,
        ),
        (
            "SL_PollSaveStatus",
            poll,
            poll_save_status_hook as *mut c_void,
            &ORIG_POLL_SAVE_STATUS,
        ),
    ];

    let mut hooks = Vec::new();
    for (name, address, detour, orig_slot) in targets {
        let hook = match unsafe { MhHook::new(address as *mut c_void, detour) } {
            Ok(hook) => hook,
            Err(status) => {
                log_message(format_args!(
                    "suppress: MhHook::new({name} @0x{address:x}) failed: {status:?} \
                     -- aborting install; a partial suppression would hang System->Quit"
                ));
                return 0;
            }
        };
        orig_slot.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            log_message(format_args!(
                "suppress: queue_enable({name}) failed: {status:?} -- aborting install"
            ));
            return 0;
        }
        hooks.push(hook);
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            // Count the SUPPRESSORS only. Folding the optional observers into this total
            // made a healthy install report "suppression hooks=3/2", which reads like a
            // broken invariant on a run where everything worked. The observers are a
            // separate, independently-optional fact and are reported as such.
            INSTALLED.store(SUPPRESSOR_HOOKS, Ordering::SeqCst);
            ARMED.store(1, Ordering::SeqCst);
            log_message(format_args!(
                "suppress: ARMED -- SL_EnqueueSaveJob @0x{enqueue:x}, \
                 SL_PollSaveStatus @0x{poll:x}, SL_ReleaseRequest @0x{release:x}; \
                 no save write job will be enqueued and every save will report success"
            ));
        }
        status => {
            log_message(format_args!("suppress: MH_ApplyQueued failed: {status:?}"));
            return 0;
        }
    }

    // OBSERVERS, applied as a SECOND batch so none of them can abort suppression. They
    // call their originals and only count; losing one costs evidence, whereas losing a
    // suppressor would hang System->Quit. (The quit-settle observer used to ride the
    // suppressor batch, where an `MhHook::new` failure on it returned 0 and disarmed
    // everything -- the opposite of what its own comment promised.)
    install_observers(settle);
    SUPPRESSOR_HOOKS
}

/// Bind ONLY the read-only observers, leaving suppression disarmed.
///
/// WHY THIS EXISTS (2026-08-04). The observers are pure diagnostics -- every one of them calls its
/// original and only counts -- but they were reachable solely from [`install`], which arms
/// suppression. Suppression is default-off in product (`save_suppression_enabled` in
/// `er-effects.toml`), so in every normal run `dispatch_observers_installed()` reported 0 and
/// `oracle_save_dispatch_last_decline_reason` reported `unsampled`. That is the one field that names
/// WHY the save lane refused, and it was unavailable in exactly the configuration users run.
///
/// It cost a wasted launch. The epoch-1 reload parks with `GameMan+0xb72`/`+0xb73` latched -- measured
/// `[+195245ms] gm-snap: save_requested=true ... b73=1`, still set at `+196171ms`, the last change-
/// detected snapshot in a log running to `+384590ms`, while epoch 0 drained the identical request in
/// 24-55ms -- and those two bytes are the `FUN_140afa6d0` case-7 gate. Without the decline reason the
/// only way to find out why is to guess, and guessing is what produced a fix that would have turned a
/// silent no-op warp into a permanent black screen.
///
/// Binds no suppressor, never sets `ARMED`, and returns the observer count so a caller can log it.
/// `MH_Initialize` is idempotent (`MH_ERROR_ALREADY_INITIALIZED` is accepted), so this composes with
/// any other MinHook user in the process.
pub fn install_observers_only() -> usize {
    if is_armed() {
        // `install` already bound them; re-binding the same prologues would double-detour.
        return dispatch_observers_installed();
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!(
                "suppress: observers-only MH_Initialize failed: {status:?} -- the save-lane refusal \
                 will stay unattributed this run"
            ));
            return 0;
        }
    }
    // The release address is what the decline sampler reads the SL request slot through; without it
    // the slot fields report null rather than a wrong number.
    if let Some(release) = verify(
        SL_RELEASE_REQUEST_RVA,
        SL_RELEASE_REQUEST_SIG,
        "SL_ReleaseRequest",
    ) {
        SL_RELEASE_REQUEST.store(release, Ordering::SeqCst);
    }
    let settle = verify(
        QUIT_PHASE_SETTLE_RVA,
        QUIT_PHASE_SETTLE_SIG,
        "QuitPhaseSettle",
    );
    install_observers(settle);
    dispatch_observers_installed()
}

/// Bind the optional observers. Never aborts, never disarms: each is attempted
/// independently and a failure is logged and counted out.
#[cfg(windows)]
fn install_observers(settle: Option<usize>) {
    // The quit-settle observer keeps its private `MhHook` (nothing else contends on
    // `FUN_14067a980`, and it is a zero-argument target the union's 4-argument shape does
    // not describe). Every failure path here is non-fatal.
    let settle_queued = match settle {
        Some(address) => match unsafe {
            MhHook::new(
                address as *mut c_void,
                quit_phase_settle_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                ORIG_QUIT_PHASE_SETTLE.store(hook.trampoline() as usize, Ordering::SeqCst);
                match unsafe { hook.queue_enable() } {
                    Ok(()) => match unsafe { MH_ApplyQueued() } {
                        MH_STATUS::MH_OK => true,
                        status => {
                            ORIG_QUIT_PHASE_SETTLE.store(0, Ordering::SeqCst);
                            log_message(format_args!(
                                "suppress: quit-settle observer MH_ApplyQueued failed: {status:?} \
                                 -- suppression stays armed, but this run cannot prove the quit \
                                 path was released"
                            ));
                            false
                        }
                    },
                    Err(status) => {
                        ORIG_QUIT_PHASE_SETTLE.store(0, Ordering::SeqCst);
                        log_message(format_args!(
                            "suppress: quit-settle observer queue_enable failed: {status:?} -- \
                             suppression stays armed, but this run cannot prove the quit path \
                             was released"
                        ));
                        false
                    }
                }
            }
            Err(status) => {
                log_message(format_args!(
                    "suppress: quit-settle observer MhHook::new @0x{address:x} failed: {status:?} \
                     -- suppression stays armed, but this run cannot prove the quit path was \
                     released"
                ));
                false
            }
        },
        None => {
            log_message(format_args!(
                "suppress: quit-settle observer NOT installed -- suppression still active, but \
                 this run cannot prove the quit path was released"
            ));
            false
        }
    };
    SETTLE_OBSERVER_INSTALLED.store(usize::from(settle_queued), Ordering::SeqCst);

    // Dispatch attribution goes through the SHARED HOOK UNION, not a private `MhHook`.
    // The product DLL already detours all three lanes for its menu/continue trace, and a
    // second `MhHook::new` on an address the same MinHook instance already owns returns
    // ALREADY_CREATED -- whichever install thread lost the race would silently have no
    // hook. The union chains handlers on one dispatcher per address, which is exactly the
    // contract that exists for this. `FUN_14067dc00` currently has no other registrant;
    // going through the union anyway costs nothing and makes a future one safe.
    let mut dispatch_bound = 0_usize;
    for (name, rva, sig, handler, orig_slot) in [
        (
            "SaveDispatchCombined",
            SAVE_DISPATCH_COMBINED_RVA,
            SAVE_DISPATCH_COMBINED_SIG,
            save_dispatch_combined_hook as UnionFn,
            &ORIG_SAVE_DISPATCH_COMBINED,
        ),
        (
            "SaveDispatchChar",
            SAVE_DISPATCH_CHAR_RVA,
            SAVE_DISPATCH_CHAR_SIG,
            save_dispatch_char_hook as UnionFn,
            &ORIG_SAVE_DISPATCH_CHAR,
        ),
        (
            "SaveDispatchSystem",
            SAVE_DISPATCH_SYSTEM_RVA,
            SAVE_DISPATCH_SYSTEM_SIG,
            save_dispatch_system_hook as UnionFn,
            &ORIG_SAVE_DISPATCH_SYSTEM,
        ),
        (
            "SaveSerializeChar",
            SAVE_SERIALIZE_CHAR_RVA,
            SAVE_SERIALIZE_CHAR_SIG,
            save_serialize_char_hook as UnionFn,
            &ORIG_SAVE_SERIALIZE_CHAR,
        ),
    ] {
        let Some(address) = verify(rva, sig, name) else {
            continue;
        };
        match unsafe { register_union_hook(address, handler, orig_slot) } {
            Ok(()) => dispatch_bound += 1,
            Err(status) => log_message(format_args!(
                "suppress: observer {name} union registration @0x{address:x} failed: {status:?} \
                 -- suppression stays armed, this run just cannot attribute that link"
            )),
        }
    }
    if let Ok(bytes) = game_rva(SAVE_SERIALIZE_BYTES_RVA as u32) {
        SERIALIZE_BYTES_ADDR.store(bytes, Ordering::SeqCst);
    }
    DISPATCH_OBSERVERS_INSTALLED.store(dispatch_bound, Ordering::SeqCst);

    // THE WRITE-COMPLETION OBSERVER (see `save_job_completion.rs`): the event that says a
    // bypassed save finished writing, without anything having to poll for it.
    let job_body_bound = install_save_job_body_observer();

    // WHICH WRITE BRANCH RAN. Deliberately NOT folded into `dispatch_bound` above: that
    // count is exported as `dispatch_observers_installed()` and documented as 0..=4, and
    // widening it would silently change what an existing oracle means.
    let write_branch_bound = install_write_branch_observers();

    log_message(format_args!(
        "suppress: observers bound -- quit-settle={}, save-dispatch attribution={dispatch_bound}/4 \
         (lanes FUN_14067b940/b750/b570 + serializer FUN_14067dc00, byte counter @0x{:x}), \
         save-job-body completion={}, write-branch attribution={write_branch_bound}/2 \
         (rebuild FUN_142413860 + in-place FUN_1424142e0); these only count, they change nothing",
        if settle_queued { "yes" } else { "NO" },
        SERIALIZE_BYTES_ADDR.load(Ordering::SeqCst),
        if job_body_bound { "yes" } else { "NO" }
    ));
}

/// Detour on `FUN_140e6fb50`.
///
/// The caller has already allocated an `SLSaveContent` into `iodev+0x10` and filled it
/// with the serialized blocks. Default (no token): we do not enqueue it. We hand it
/// straight to the game's own teardown -- the exact call the native code makes when the
/// enqueue fails -- and then report success, which is the one thing the native failure
/// path does not do. With a bypass token armed: the FIRST enqueue consumes the token
/// and is forwarded to the real trampoline, so the game performs a genuine submit and a
/// genuine write; the completion watch then tells the poll detour to latch the outcome.
///
/// Releasing through `FUN_140e6f200` is not optional on the swallow path: leaving
/// `iodev+0x10` populated would permanently fail the `iodev+0x10 == 0 && iodev+0x20 == 0`
/// precondition on every later submit, and would leave the status poll dereferencing a
/// null job.
#[cfg(windows)]
unsafe extern "system" fn enqueue_save_job_hook(iodev: usize, opcode: u32) -> u8 {
    if !is_armed() {
        // Expected in a disarmed positive-control run, and a hard failure in any other:
        // this submit writes a real save. Reported on the same throttle as a swallow --
        // loudly on the first, then at milestones -- because the first occurrence is
        // what flips `suppress_submits_passed_through` off zero, and that is the gate.
        let count = SUBMITS_PASSED_THROUGH.fetch_add(1, Ordering::SeqCst) + 1;
        if should_report(count, false) {
            log_message(format_args!(
                "suppress: save submit #{count} PASSED THROUGH (opcode={opcode}) -- \
                 suppression is not armed, this save is being written for real"
            ));
            publish_snapshot();
        }
        let orig = ORIG_ENQUEUE_SAVE_JOB.load(Ordering::SeqCst);
        if orig == 0 {
            return 0;
        }
        let original: EnqueueSaveJobFn = unsafe { core::mem::transmute(orig) };
        return unsafe { original(iodev, opcode) };
    }

    // ONE-SHOT BYPASS: consume a pending token and forward this submit for REAL. This
    // is the sanctioned Save Game write -- the only save that is allowed to reach disk.
    // Logged and published unconditionally: it is a rare, user-initiated event, and
    // its failure modes must never be quieter than its success (noise rule 3).
    if BYPASS_TOKEN
        .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // Baseline the worker's completion counter BEFORE the submit, so the adopter can
        // only ever accept a job that finished after this call.
        arm_save_job_completion_watch();
        BYPASS_COMPLETION_WATCH.store(1, Ordering::SeqCst);
        let count = BYPASS_ALLOWED_TOTAL.fetch_add(1, Ordering::SeqCst) + 1;
        let orig = ORIG_ENQUEUE_SAVE_JOB.load(Ordering::SeqCst);
        if orig == 0 {
            // Unreachable via `install` (armed implies the trampoline bound); kept loud
            // because this exact path failing silently would eat the user's one save.
            BYPASS_ALLOWED_FAILED_TOTAL.fetch_add(1, Ordering::SeqCst);
            latch_submit_failure_as_final_status("enqueue trampoline unset");
            log_message(format_args!(
                "suppress: BUG -- bypass allow #{count} with enqueue trampoline unset; \
                 the user's save was NOT submitted"
            ));
            publish_snapshot();
            return 0;
        }
        log_message(format_args!(
            "suppress: bypass ALLOW #{count} -- forwarding save submit for real \
             (iodev=0x{iodev:x}, opcode={opcode}); this is the user's explicit Save Game write"
        ));
        let original: EnqueueSaveJobFn = unsafe { core::mem::transmute(orig) };
        let submitted = unsafe { original(iodev, opcode) };
        if submitted == 0 {
            let failed = BYPASS_ALLOWED_FAILED_TOTAL.fetch_add(1, Ordering::SeqCst) + 1;
            // No job was queued, so no body will ever run and no poll will ever go
            // terminal: this commit's outcome is already decided and waiting out a watchdog
            // would only delay reporting it. Latch the failure here, at the one place that
            // saw it, instead of inferring it later from silence.
            latch_submit_failure_as_final_status("native enqueue returned 0");
            log_message(format_args!(
                "suppress: bypass allow #{count} FAILED -- native enqueue returned 0 \
                 (failure #{failed}); the user's save was NOT submitted"
            ));
        }
        publish_snapshot();
        return submitted;
    }

    let release = SL_RELEASE_REQUEST.load(Ordering::SeqCst);
    if release == 0 {
        // Never reachable via `install`, which refuses to arm without the release
        // address. Counted rather than assumed away: passing the submit through here
        // writes the save, which is a louder failure than a silent leak.
        // Unthrottled: this writes a real save, and `install` refuses to arm without the
        // release address so it is unreachable anyway. Throttling a bug path is the wrong
        // default even when the throttle would never engage.
        let count = RELEASE_UNAVAILABLE.fetch_add(1, Ordering::SeqCst) + 1;
        log_message(format_args!(
            "suppress: BUG -- armed with no release address, save submit #{count} \
             passed through and will be written"
        ));
        publish_snapshot();
        let orig = ORIG_ENQUEUE_SAVE_JOB.load(Ordering::SeqCst);
        if orig == 0 {
            return 0;
        }
        let original: EnqueueSaveJobFn = unsafe { core::mem::transmute(orig) };
        return unsafe { original(iodev, opcode) };
    }

    // Sample the request slot on BOTH sides of the release. The swallow's whole contract
    // is "leave the device exactly as the native enqueue-failure path would", and the
    // consequence of getting that wrong is not a lost save but a PERMANENT one: the submit
    // builders gate on `iodev+0x10 == 0 && iodev+0x20 == 0`, so a field left populated
    // refuses every later save forever. Measuring it is two `ReadProcessMemory` calls per
    // swallow, and swallows are rare.
    let before = read_sl_slot();
    store_slot_sample(before, &SWALLOW_SLOT_BEFORE);
    if let Some(singleton) = read_sl_iodev()
        && singleton != iodev
    {
        SWALLOW_IODEV_MISMATCH.fetch_add(1, Ordering::SeqCst);
        log_message(format_args!(
            "suppress: BUG -- swallow handed iodev=0x{iodev:x} but the SL singleton is \
             0x{singleton:x}; the release is being applied to a different object than the \
             submit builders check"
        ));
    }

    let released: ReleaseRequestFn = unsafe { core::mem::transmute(release) };
    unsafe { released(iodev) };

    let after = read_sl_slot();
    store_slot_sample(after, &SWALLOW_SLOT_AFTER);
    let left_dirty = after.is_some_and(|slot| !slot.admits_a_save());
    if left_dirty {
        let dirty = SWALLOW_RELEASE_LEFT_DIRTY.fetch_add(1, Ordering::SeqCst) + 1;
        // Unthrottled on purpose. This is not a rate, it is a latch: from here on every
        // save in the process is refused by a precondition WE left failing.
        log_message(format_args!(
            "suppress: BUG -- FUN_140e6f200 did not clear the request slot (#{dirty}); \
             before {}, after {} -- the submit precondition `iodev+0x10 == 0 && \
             iodev+0x20 == 0` now fails for every later save",
            describe_slot(before),
            describe_slot(after)
        ));
        publish_snapshot();
    }

    let count = SUBMITS_SWALLOWED.fetch_add(1, Ordering::SeqCst) + 1;
    // Swallowing is the EXPECTED steady state, not an event. It is counted in telemetry
    // (`suppress_submits_swallowed`), which is what a harness reads; the log only needs
    // to show that it started, that it kept happening, and any new kind of save.
    let novel = note_opcode(opcode);
    if should_report(count, novel) {
        // The old form of this line asserted "request released" as a fixed string, which
        // read like evidence and was not: it printed identically whether the slot had been
        // cleared or left populated. Print the MEASURED post-release state instead -- a
        // reader chasing a permanently-refusing save needs to know which it was.
        log_message(format_args!(
            "suppress: swallowed save submit #{count} (iodev=0x{iodev:x}, opcode={opcode}) \
             -- no job enqueued, reporting submitted; slot after release: {}",
            describe_slot(after)
        ));
        // Publish on the same schedule. A snapshot per swallow meant a full JSON
        // re-serialize, `fs::write` and `fs::rename` on the GAME thread for every save
        // request -- this detours `FUN_140e6fb50`, whose callers are the per-frame
        // dispatchers, strictly above the `FUN_14240ae10` worker boundary -- and each of
        // those can re-enter the host DLL's own file-API detours. Every counter a
        // harness gates on is a threshold, and a threshold is crossed on the first
        // occurrence, which is always published.
        publish_snapshot();
    }
    1
}

/// Detour on `FUN_140e6e430`.
///
/// Always runs the original first. The original's answer is the guard: only the literal
/// "no request" code is rewritten, and that code is produced by exactly one branch,
/// the `iodev+0x10 == 0` early-out. Any genuinely outstanding IO -- a save we did not
/// swallow, or a load, which lives in `iodev+0x18` -- cannot produce it, so it cannot
/// be lied about.
///
/// Bypass completion watch: after a token-forwarded submit, the first poll answer that
/// is not "in flight" is the real save's terminal outcome; latch it for the caller.
/// `decide_status` itself is untouched -- a real in-flight save returns 0/1/2/7/8/9,
/// none of which the structural 4-only rewrite ever touches.
#[cfg(windows)]
unsafe extern "system" fn poll_save_status_hook(iodev: usize) -> u32 {
    let orig = ORIG_POLL_SAVE_STATUS.load(Ordering::SeqCst);
    if orig == 0 {
        return SL_STATUS_NO_REQUEST;
    }
    let original: PollSaveStatusFn = unsafe { core::mem::transmute(orig) };
    let raw = unsafe { original(iodev) };

    if BYPASS_COMPLETION_WATCH.load(Ordering::SeqCst) != 0
        && raw != SL_STATUS_IN_FLIGHT
        && BYPASS_COMPLETION_WATCH
            .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        // RAW, never the rewritten value: `decide_status` turns a 4 ("no request") into a
        // 0 ("success") for the game's benefit, and adopting that here would report a
        // commit whose request had already vanished as a successful save.
        BYPASS_FINAL_STATUS.store(raw, Ordering::SeqCst);
        BYPASS_FINAL_STATUS_SOURCE.store(BYPASS_STATUS_SOURCE_POLL, Ordering::SeqCst);
        BYPASS_FINAL_STATUS_FRESH.store(1, Ordering::SeqCst);
        BYPASS_COMPLETED_VIA_POLL_TOTAL.fetch_add(1, Ordering::SeqCst);
        log_message(format_args!(
            "suppress: bypassed save terminal status={raw} (0=success), observed by a native poll consumer"
        ));
        publish_snapshot();
    }

    let decided = decide_status(raw, is_armed(), SUBMITS_SWALLOWED.load(Ordering::SeqCst));
    if decided == raw {
        STATUS_PASSED_THROUGH.fetch_add(1, Ordering::SeqCst);
    } else {
        STATUS_FAKED.fetch_add(1, Ordering::SeqCst);
        // Split off the rewrites that retired nothing. The rewrite only DOES something
        // when the game believes a save is in flight (`saveState != 0`, which only the
        // dispatcher's commit tail sets, i.e. only after a swallow); every other rewrite
        // answers an idle poll where 4 and 0 are equivalent to the caller. Without this
        // split `suppress_status_faked` is dominated by idle polls and reads as activity
        // when there was none. Failing to read the field counts as idle: it is the
        // conservative side (it never inflates the "this mattered" number).
        if read_save_state().unwrap_or(0) == 0 {
            STATUS_FAKED_IDLE.fetch_add(1, Ordering::SeqCst);
        }
    }
    sample_quit_phase();
    decided
}

/// Observer on `FUN_14067b940`, the COMBINED (b72 && b73) save dispatch lane -- the lane the
/// Save Game commit deliberately produces by firing both native request setters.
#[cfg(windows)]
unsafe extern "system" fn save_dispatch_combined_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    unsafe { observe_dispatch(&ORIG_SAVE_DISPATCH_COMBINED, SAVE_LANE_COMBINED, a, b, c, d) }
}

/// Observer on `FUN_14067b750`, the character-slot-only (b72, !b73) lane.
#[cfg(windows)]
unsafe extern "system" fn save_dispatch_char_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    unsafe { observe_dispatch(&ORIG_SAVE_DISPATCH_CHAR, SAVE_LANE_CHAR, a, b, c, d) }
}

/// Observer on `FUN_14067b570`, the system-slot-only (!b72, b73) lane.
#[cfg(windows)]
unsafe extern "system" fn save_dispatch_system_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    unsafe { observe_dispatch(&ORIG_SAVE_DISPATCH_SYSTEM, SAVE_LANE_SYSTEM, a, b, c, d) }
}

/// Shared body of the three dispatch observers: forward verbatim, count, and say ONCE per
/// armed bypass when the lane refuses.
///
/// A refusal (`AL == 0`) is the failure this instrument exists for. The lane touches
/// nothing on that path -- `GameMan+0xb72`/`+0xb73` stay set and `saveState` stays 0 -- so
/// the dispatcher re-enters it on the very next frame and the refusal repeats for as long
/// as the request is latched. That is why the "the dispatcher refused the user's save" line
/// is emitted once per arm rather than per call, and why the aggregate uses `should_report`.
///
/// All four register arguments are forwarded verbatim (the lanes take three; the fourth is
/// the union's shared shape and the callee ignores it), and the return is forwarded whole:
/// the game only tests AL, but preserving RAX keeps the detour a true no-op.
#[cfg(windows)]
unsafe fn observe_dispatch(
    orig_slot: &AtomicUsize,
    lane: usize,
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let orig = orig_slot.load(Ordering::SeqCst);
    if orig == 0 {
        // Unreachable once bound. Refusing here would silently kill every save in the
        // game, so say so rather than inventing a return value.
        log_message(format_args!(
            "suppress: BUG -- save dispatch lane {lane} observer ran with no trampoline; \
             reporting 'not dispatched' to the game"
        ));
        return 0;
    }
    let original: UnionFn = unsafe { core::mem::transmute(orig) };
    let ret = unsafe { original(a, b, c, d) };
    let calls = DISPATCH_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    DISPATCH_LAST_LANE.store(lane, Ordering::SeqCst);
    if ret & 0xff != 0 {
        return ret;
    }
    let declines = DISPATCH_DECLINES.fetch_add(1, Ordering::SeqCst) + 1;
    // Sample the SL request slot at the decline. The lane touched nothing on this path, so
    // what we read here IS what the submit builder's precondition saw -- and since the
    // builder's other four operands are statically guaranteed by the call site (see the
    // SL REQUEST SLOT block), `iodev+0x10` and `iodev+0x20` are the only two operands that
    // can have failed. One decline therefore names the culprit outright.
    let slot = read_sl_slot();
    store_slot_sample(slot, &DECLINE_SLOT);
    let reason = classify_sl_bail(slot);
    DECLINE_BAIL_REASON.store(reason, Ordering::SeqCst);
    if bypass_pending() {
        DISPATCH_DECLINES_WITH_BYPASS.fetch_add(1, Ordering::SeqCst);
        if BYPASS_DECLINE_REPORTED.swap(1, Ordering::SeqCst) == 0 {
            log_message(format_args!(
                "suppress: the native save dispatcher REFUSED the user's Save Game request \
                 (lane={lane}, decline #{declines} of {calls} dispatch entries) -- the request \
                 flags reached the dispatcher and it returned 0 without building a submit, so \
                 no SL enqueue can ever arrive and the one-shot bypass will expire. The failure \
                 is UPSTREAM of the enqueue; serializer calls={} failures={} \
                 last_fail_bytes={} last_fail_step={}. SL request slot: {}. Verdict: {}",
                SERIALIZE_CALLS.load(Ordering::SeqCst),
                SERIALIZE_FAILURES.load(Ordering::SeqCst),
                SERIALIZE_LAST_FAIL_BYTES.load(Ordering::SeqCst),
                serialize_last_fail_step(),
                describe_slot(slot),
                bail_verdict(reason)
            ));
            publish_snapshot();
        }
    } else if should_report(declines, false) {
        log_message(format_args!(
            "suppress: save dispatch lane {lane} declined (#{declines} of {calls} entries) -- \
             request flags stay latched, no submit built. SL request slot: {}",
            describe_slot(slot)
        ));
    }
    ret
}

/// Turn a `SL_BAIL_*` code into the sentence that says what to do about it.
///
/// Kept beside the classifier rather than in the caller so the decline log line and the
/// telemetry oracle can never drift apart on what a code means.
pub fn bail_verdict(reason: usize) -> &'static str {
    match reason {
        SL_BAIL_IODEV_UNREADABLE => {
            "the SL device could not be read, so this decline attributes nothing -- check \
             slot_read_failures before believing any iodev oracle in this run"
        }
        SL_BAIL_SAVE_CONTENT_LATCHED | SL_BAIL_SAVE_CONTENT_AND_JOB_LATCHED => {
            "a previous SAVE request is still latched at iodev+0x10. Only FUN_140e6f200 \
             clears it, so either a swallow's release did not run (check \
             swallow_release_left_dirty and release_unavailable) or a builder took the \
             iodev+0x28 DEFERRED branch, which returns success without ever calling \
             FUN_140e6fb50 and leaves +0x10 populated for FUN_140e6f370 to replay"
        }
        SL_BAIL_LOAD_JOB_LATCHED => {
            "a completed LOAD is still occupying the shared job slot. FUN_140e6e080 case \
             0x14 deliberately does not release on success; the consumer FUN_14067b100 -> \
             FUN_140e6e380 -> FUN_140e6f200 does. This is NOT the swallow's doing -- the \
             load was never consumed, so every save is blocked by the load's job"
        }
        SL_BAIL_JOB_LATCHED => {
            "a job is latched at iodev+0x20 with no content on either side -- an enqueue \
             whose poll never reached a terminal case, so nothing ever called \
             FUN_140e6f200"
        }
        SL_BAIL_PRECONDITION_CLEAR => {
            "the precondition HOLDS, so the builder passed its guard: the refusal is the \
             NetworkHeap HeapAlloc(0x298)/SLSaveContent construction, or the lane bailed \
             before reaching the builder at all"
        }
        _ => "no decline has been classified yet",
    }
}

/// Observer on `FUN_14067dc00`, the character serializer.
///
/// Its return value is the SOLE gate on the submit call in both character lanes, so a zero
/// here is a character save that produced no submit at all.
///
/// The byte counter `_DAT_143d69920` is read ONCE, after the call, and decoded into a step
/// name. It is not compared against a pre-call sample: the serializer's only exit that
/// leaves the counter untouched is a first gate proven unreachable here, and a pre/post
/// comparison actively misreads the normal case -- the lane is re-entered every frame while
/// the request stays latched, aborts at the same step each time, and therefore stores an
/// identical count, which a delta test reports as "did not move".
#[cfg(windows)]
unsafe extern "system" fn save_serialize_char_hook(
    game_man: usize,
    buffer: usize,
    size: usize,
    out_bytes: usize,
) -> usize {
    let orig = ORIG_SAVE_SERIALIZE_CHAR.load(Ordering::SeqCst);
    if orig == 0 {
        log_message(format_args!(
            "suppress: BUG -- character-serializer observer ran with no trampoline; \
             reporting 'serialize failed' to the game"
        ));
        return 0;
    }
    let original: UnionFn = unsafe { core::mem::transmute(orig) };
    let ret = unsafe { original(game_man, buffer, size, out_bytes) };
    let calls = SERIALIZE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    if ret & 0xff != 0 {
        return ret;
    }
    let failures = SERIALIZE_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
    let recorded = read_serialize_bytes()
        .map(|bytes| bytes as u64)
        .unwrap_or(SAVE_SERIALIZE_BYTES_UNREADABLE);
    SERIALIZE_LAST_FAIL_BYTES.store(recorded, Ordering::SeqCst);
    if should_report(failures, false) {
        log_message(format_args!(
            "suppress: character serializer FUN_14067dc00 returned 0 (failure #{failures} of \
             {calls} calls) at {} -- {recorded} (0x{recorded:x}) bytes produced: {}. No submit \
             is built for this save, so the request flags stay latched and no SL enqueue is \
             created",
            serialize_fail_step_label(recorded),
            serialize_fail_step_detail(recorded)
        ));
        publish_snapshot();
    }
    ret
}

/// Read the character serializer's byte counter, or `None` when it is not resolvable.
#[cfg(windows)]
fn read_serialize_bytes() -> Option<usize> {
    use er_game_base::mem::safe_read_usize;

    let addr = SERIALIZE_BYTES_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return None;
    }
    unsafe { safe_read_usize(addr) }
}

/// Read `GameMan+0xb80` (`saveState`), or `None` when it is not reachable.
#[cfg(windows)]
fn read_save_state() -> Option<u32> {
    use er_game_base::{mem::safe_read_usize, rva::GAME_MAN_SINGLETON_RVA};

    const GAME_MAN_SAVE_STATE_B80_OFFSET: usize = 0xb80;

    let base = er_game_base::mem::game_module_base().ok()?;
    let game_man = unsafe { safe_read_usize(base + GAME_MAN_SINGLETON_RVA) }?;
    if game_man < 0x10000 {
        return None;
    }
    let raw = unsafe { safe_read_usize(game_man + GAME_MAN_SAVE_STATE_B80_OFFSET) }?;
    Some((raw & 0xffff_ffff) as u32)
}

/// Observer on `FUN_14067a980`, the sole performer of the `bc4` 2 -> 3 transition.
///
/// Pure observation: the original runs unmodified and its effect is untouched. Verified
/// against the 1.16.2 dump -- `undefined FUN_14067a980(void)`, 27 bytes, no parameters,
/// body exactly `if (bc4 == 2) bc4 = 3;` -- so the zero-argument detour signature is
/// correct and the original is called before any of our code can clobber a register.
///
/// It counts the TRANSITION, not the call, and that distinction is the whole value of
/// the instrument. `DoSaveStuff` calls this function from case 0 and from cases 3, 7 and
/// 9 of its switch on the *save status* -- nothing there tests `bc4` -- and the menu job
/// `FUN_1407ecf20` calls it from *its own* state 3. So it runs on every ordinary save
/// completion, when `bc4` is 0 and the body is a no-op.
///
/// Counting entries would therefore make `quit_phase_settle_events` non-zero from the
/// first rune the player picked up, on a run where no quit ever happened -- a FALSE PASS
/// on the one oracle that exists to catch the quit deadlock. That is the same "the
/// instrument does not measure what it claims" failure as sampling the transient value,
/// one level further in, and in the more dangerous direction.
///
/// A failed read fails CLOSED (no count): under-counting yields a loud false FAIL that
/// gets investigated, while over-counting would ship a hang as a pass.
#[cfg(windows)]
unsafe extern "system" fn quit_phase_settle_hook() {
    // Read BEFORE the original runs: afterwards the 2 is gone and the transition is
    // indistinguishable from having arrived already-3.
    let settles = read_quit_phase() == Some(QUIT_PHASE_SAVE_SUBMITTED);
    let orig = ORIG_QUIT_PHASE_SETTLE.load(Ordering::SeqCst);
    if orig != 0 {
        let original: unsafe extern "system" fn() = unsafe { core::mem::transmute(orig) };
        unsafe { original() };
    }
    if !settles {
        return;
    }
    let count = QUIT_PHASE_SETTLE_EVENTS.fetch_add(1, Ordering::SeqCst) + 1;
    if should_report(count, false) {
        log_message(format_args!(
            "suppress: quit-to-title wait job released (bc4 2 -> 3), settle event #{count}"
        ));
    }
    // Flush on every settle, not on the milestone schedule. This is the moment the
    // acceptance test is about -- the player quit to title and the game did not hang --
    // so the on-disk telemetry must be current here even if the process is killed
    // immediately afterwards. It is inherently rare: once per quit, not once per save.
    publish_snapshot();
}

/// Sample `GameMan+0xbc4` from the save-status poll detour.
///
/// Driven ONLY from the poll, which is rare and already save-related. It was once
/// driven from the census `CreateFileW` detour as well, on the theory that sampling
/// more often would eventually catch `bc4 == 3`. That was wrong twice over: each call
/// costs a `GetModuleHandleA` plus two `ReadProcessMemory` syscalls, paid on *every
/// file open in the process*, and it still could not catch the transition, because
/// `FUN_14067a980` sets 3 and the quit chain consumes and resets it within the same
/// sequence. The transition is counted as an event instead.
#[cfg(windows)]
fn sample_quit_phase() {
    if let Some(phase) = read_quit_phase() {
        QUIT_PHASE_MAX_SEEN.fetch_max(phase, Ordering::SeqCst);
    }
}

/// Read `GameMan+0xbc4`, the return-to-title phase, or `None` if it is not reachable.
///
/// Split out from `sample_quit_phase` because the settle observer needs the *value*
/// rather than the running maximum: it has to know whether the call it is intercepting
/// will actually perform the 2 -> 3 transition.
#[cfg(windows)]
fn read_quit_phase() -> Option<usize> {
    use er_game_base::{mem::safe_read_usize, rva::GAME_MAN_SINGLETON_RVA};

    const GAME_MAN_QUIT_PHASE_BC4_OFFSET: usize = 0xbc4;

    let base = er_game_base::mem::game_module_base().ok()?;
    let game_man = unsafe { safe_read_usize(base + GAME_MAN_SINGLETON_RVA) }?;
    if game_man < 0x10000 {
        return None;
    }
    let raw = unsafe { safe_read_usize(game_man + GAME_MAN_QUIT_PHASE_BC4_OFFSET) }?;
    Some((raw & 0xff) as usize)
}

include!("save_job_completion.rs");
include!("save_write_branch.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_request_becomes_success_once_a_submit_was_swallowed() {
        assert_eq!(
            decide_status(SL_STATUS_NO_REQUEST, true, 1),
            SL_STATUS_SUCCESS
        );
    }

    #[test]
    fn nothing_is_rewritten_before_the_first_swallow() {
        // Until we have actually suppressed something there is no fake success to
        // report, and the suppression must be inert.
        assert_eq!(
            decide_status(SL_STATUS_NO_REQUEST, true, 0),
            SL_STATUS_NO_REQUEST
        );
    }

    #[test]
    fn nothing_is_rewritten_when_disarmed() {
        assert_eq!(
            decide_status(SL_STATUS_NO_REQUEST, false, 5),
            SL_STATUS_NO_REQUEST
        );
    }

    #[test]
    fn every_other_status_is_passed_through_untouched() {
        // 1 = in flight, 2 = hard failure (popup), 7/9 = done-not-success, 8 = error.
        // Rewriting any of these would either mask a real in-flight job or invent an
        // outcome the game did not reach.
        for raw in [0_u32, 1, 2, 3, 5, 6, 7, 8, 9, 10, 0xffff_ffff] {
            assert_eq!(
                decide_status(raw, true, 99),
                raw,
                "status {raw} was rewritten"
            );
        }
    }

    #[test]
    fn a_job_result_only_maps_to_success_when_it_is_literally_zero() {
        // 0 is the value `FUN_14240ebd0` writes at construction and no failure path
        // overwrites. Everything else -- including the "we could not read it" stand-in --
        // has to come out as a non-success status, or a save that did not land could be
        // reported as one that did.
        assert_eq!(save_job_result_to_status(0), SL_STATUS_SUCCESS);
        for result in [
            1_u32,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
            9,
            0x14,
            SAVE_JOB_RESULT_UNREADABLE,
        ] {
            assert_ne!(
                save_job_result_to_status(result),
                SL_STATUS_SUCCESS,
                "job result {result} was mapped to success"
            );
        }
    }

    #[test]
    fn job_results_map_through_the_polls_own_terminal_table() {
        // `FUN_140e6e430` case 0x14, verbatim.
        assert_eq!(save_job_result_to_status(3), 7);
        assert_eq!(save_job_result_to_status(4), 8);
        assert_eq!(save_job_result_to_status(2), 8);
        assert_eq!(save_job_result_to_status(7), 2);
        assert_eq!(save_job_result_to_status(5), 9);
    }

    #[test]
    fn adoption_does_nothing_without_a_live_completion_watch() {
        // The watch is only set when our one-shot token was consumed by a real enqueue.
        // Without it there is no commit to complete, so a job completion from anywhere
        // else must not be able to latch a status.
        BYPASS_COMPLETION_WATCH.store(0, Ordering::SeqCst);
        SAVE_JOB_COMPLETIONS.store(99, Ordering::SeqCst);
        SAVE_JOB_COMPLETIONS_AT_ALLOW.store(0, Ordering::SeqCst);
        assert_eq!(adopt_completed_save_job_as_final_status(), None);
    }

    #[test]
    fn prologue_guard_accepts_exact_and_longer_reads() {
        assert!(prologue_matches(&[0x40, 0x53, 0x56], &[0x40, 0x53, 0x56]));
        assert!(prologue_matches(
            &[0x40, 0x53, 0x56, 0x57],
            &[0x40, 0x53, 0x56]
        ));
    }

    #[test]
    fn prologue_guard_rejects_drift_and_short_reads() {
        assert!(!prologue_matches(&[0x40, 0x53, 0x99], &[0x40, 0x53, 0x56]));
        assert!(!prologue_matches(&[0x40, 0x53], &[0x40, 0x53, 0x56]));
    }

    #[test]
    fn the_first_occurrence_is_always_reported() {
        // The threshold every harness gate depends on: a counter crossing 0 -> 1 must
        // reach both the log and a published snapshot, or a gate could read a stale
        // zero for something that did happen.
        assert!(should_report(1, false));
    }

    #[test]
    fn repeats_collapse_to_exponential_milestones() {
        let reported: Vec<u64> = (1..=64).filter(|n| should_report(*n, false)).collect();
        assert_eq!(reported, vec![1, 2, 4, 8, 16, 32, 64]);
    }

    #[test]
    fn a_novel_event_is_reported_however_late_it_appears() {
        // A save opcode never seen before is a different KIND of save reaching the
        // choke point -- exactly what the census exists to discover. Throttling must
        // never be able to hide one.
        assert!(should_report(9_999, true));
        assert!(!should_report(9_999, false));
    }

    #[test]
    fn throttling_stays_sublinear_at_measured_save_volumes() {
        // Calibrated on the MEASURED rate: live runs report 7-25 swallowed submits per
        // session. Anchored at the top of that range rather than an invented one.
        let lines = (1..=25_u64).filter(|n| should_report(*n, false)).count();
        assert_eq!(lines, 5, "25 swallows should cost 5 lines, not 25");
        // Still sublinear if a session ever runs far longer than any measured so far.
        let far = (1..=10_000_u64)
            .filter(|n| should_report(*n, false))
            .count();
        assert_eq!(far, 14);
    }

    #[test]
    fn each_opcode_is_novel_exactly_once() {
        // Uses opcodes no other test touches: SEEN_OPCODES is process-global state.
        assert!(note_opcode(11));
        assert!(!note_opcode(11));
        assert!(note_opcode(12));
        assert!(!note_opcode(12));
    }

    #[test]
    fn opcodes_past_the_mask_share_the_catch_all_bit() {
        // Documented collapse: >= 63 is reported novel once, not once per opcode.
        assert!(note_opcode(64));
        assert!(!note_opcode(9_999));
    }

    #[test]
    fn recorded_signatures_are_the_verified_1162_prologues() {
        // Guards against an edit that silently shortens or reorders a signature: these
        // exact bytes were read out of `eldenring-deobf.bin` at the hook addresses.
        assert_eq!(&SL_ENQUEUE_SAVE_JOB_SIG[..4], &[0x40, 0x53, 0x56, 0x57]);
        assert_eq!(&SL_POLL_SAVE_STATUS_SIG[..4], &[0x40, 0x57, 0x48, 0x83]);
        assert_eq!(&SL_RELEASE_REQUEST_SIG[..4], &[0x48, 0x89, 0x6C, 0x24]);
        assert!(SL_ENQUEUE_SAVE_JOB_SIG.len() >= 16);
        assert!(SL_POLL_SAVE_STATUS_SIG.len() >= 16);
        assert!(SL_RELEASE_REQUEST_SIG.len() >= 16);
        // `SaveLoad2::SLSaveSession`'s job body at 0x14240fd70, read the same way:
        // `mov rax,rsp; push rbp; push rdi; push r12; push r14; push r15`. Six whole
        // instructions before MinHook's 5-byte window closes, and no relative branch.
        assert_eq!(
            &SL_SAVE_JOB_BODY_SIG[..11],
            &[
                0x48, 0x8B, 0xC4, 0x55, 0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57
            ]
        );
        assert!(SL_SAVE_JOB_BODY_SIG.len() >= 16);
    }

    #[test]
    fn dispatch_observer_signatures_are_the_verified_1162_prologues() {
        // Read out of `eldenring-deobf.bin` at 0x14067b940 / b750 / b570 / dc00 and
        // cross-checked against the 1.16.2 Ghidra dump at the same VAs (shift 0).
        assert_eq!(&SAVE_DISPATCH_COMBINED_SIG[..3], &[0x48, 0x8B, 0xC4]);
        assert_eq!(
            &SAVE_DISPATCH_CHAR_SIG[..5],
            &[0x48, 0x89, 0x5C, 0x24, 0x20]
        );
        assert_eq!(&SAVE_DISPATCH_SYSTEM_SIG[..4], &[0x48, 0x8B, 0xC4, 0x57]);
        assert_eq!(
            &SAVE_SERIALIZE_CHAR_SIG[..5],
            &[0x40, 0x55, 0x53, 0x56, 0x57]
        );
        // MinHook relocates the first 5 bytes; each of these decodes to whole instructions
        // across at least that window and contains no relative branch.
        for sig in [
            SAVE_DISPATCH_COMBINED_SIG,
            SAVE_DISPATCH_CHAR_SIG,
            SAVE_DISPATCH_SYSTEM_SIG,
            SAVE_SERIALIZE_CHAR_SIG,
        ] {
            assert!(sig.len() >= 16);
        }
    }

    #[test]
    fn write_branch_observer_signatures_are_the_verified_1162_prologues() {
        // Read out of `eldenring-deobf.bin` at 0x142413860 / 0x1424142e0 (file offset == RVA)
        // and cross-checked against the 1.16.2 Ghidra dump at the same VAs (shift 0). Both
        // open with the same seven-instruction multi-push prologue.
        const SHARED_PUSHES: &[u8] = &[
            0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
        ];
        assert_eq!(&SAVE_WRITE_FULL_REBUILD_SIG[..12], SHARED_PUSHES);
        assert_eq!(&SAVE_WRITE_IN_PLACE_SIG[..12], SHARED_PUSHES);
        // ...and diverge at byte 14, where the frame pointer is set up: the rebuild takes
        // `lea rbp,[rsp-0x60]` (8d 6c 24 a0) and the patcher `lea rbp,[rsp-0xd0]`
        // (8d ac 24 30 ff ff ff). A signature that lost this would match both functions.
        assert_eq!(
            &SAVE_WRITE_FULL_REBUILD_SIG[12..17],
            &[0x48, 0x8D, 0x6C, 0x24, 0xA0]
        );
        assert_eq!(
            &SAVE_WRITE_IN_PLACE_SIG[12..17],
            &[0x48, 0x8D, 0xAC, 0x24, 0x30]
        );
        assert_ne!(SAVE_WRITE_FULL_REBUILD_SIG, SAVE_WRITE_IN_PLACE_SIG);
        // MinHook relocates the first 5 bytes; each of these decodes to whole instructions
        // across at least that window and contains no relative branch.
        for sig in [SAVE_WRITE_FULL_REBUILD_SIG, SAVE_WRITE_IN_PLACE_SIG] {
            assert!(sig.len() >= 16);
        }
    }

    #[test]
    fn a_missing_write_trampoline_reports_failure_not_success() {
        // The whole point of `SAVE_WRITE_FAILED_RESULT`: the job body treats 0 as SUCCESS
        // (`FUN_14240d8d0(job) == 0` is its continue condition), so a degraded observer must
        // never return 0 -- that would certify a write that never happened.
        assert_ne!(SAVE_WRITE_FAILED_RESULT, 0);
        assert_eq!(SAVE_WRITE_FAILED_RESULT, 6);
    }

    #[test]
    fn the_serializer_exit_map_matches_the_1162_decompile() {
        // Boundaries read out of FUN_14067dc00: two fixed 0x10-byte writes
        // (0x14067dd3d header, 0x14067ddcc xorshift seeds) then FUN_140257f20, whose
        // length is game-state dependent. Anything past 0x20 is deliberately one bucket;
        // widening it would be a claim the decompile does not support.
        assert_eq!(
            serialize_fail_step_label(0),
            SAVE_SERIALIZE_STEP_HEADER_NONE
        );
        assert_eq!(
            serialize_fail_step_label(0x0f),
            SAVE_SERIALIZE_STEP_HEADER_SHORT
        );
        assert_eq!(
            serialize_fail_step_label(0x10),
            SAVE_SERIALIZE_STEP_RANDSEED_NONE
        );
        assert_eq!(
            serialize_fail_step_label(0x1f),
            SAVE_SERIALIZE_STEP_RANDSEED_SHORT
        );
        assert_eq!(
            serialize_fail_step_label(0x20),
            SAVE_SERIALIZE_STEP_GAMEDATAMAN_NONE
        );
        assert_eq!(
            serialize_fail_step_label(0x21),
            SAVE_SERIALIZE_STEP_AFTER_OUTPUT
        );
        assert_eq!(
            serialize_fail_step_label(0x280000),
            SAVE_SERIALIZE_STEP_AFTER_OUTPUT
        );
        // The sentinel is a read failure, NOT the largest byte count.
        assert_eq!(
            serialize_fail_step_label(SAVE_SERIALIZE_BYTES_UNREADABLE),
            SAVE_SERIALIZE_STEP_UNREADABLE
        );
        // Every bucket has its own long form, and none of them can break the hand-built
        // telemetry JSON.
        let labels = [
            SAVE_SERIALIZE_BYTES_UNREADABLE,
            0,
            0x08,
            0x10,
            0x18,
            0x20,
            0x100,
        ];
        for bytes in labels {
            let label = serialize_fail_step_label(bytes);
            let detail = serialize_fail_step_detail(bytes);
            assert!(!label.is_empty() && !detail.is_empty());
            for s in [label, detail] {
                assert!(!s.contains('"'), "{s} would break the telemetry JSON");
                assert!(!s.contains('\\'), "{s} would break the telemetry JSON");
                assert!(!s.contains('\n'), "{s} would break the telemetry JSON");
            }
        }
    }

    #[test]
    fn the_lanes_have_distinct_codes_and_none_is_the_zero_sentinel() {
        // The lane code lands in telemetry as a number, so a collision would silently
        // misattribute which dispatcher the game last entered.
        let lanes = [SAVE_LANE_CHAR, SAVE_LANE_SYSTEM, SAVE_LANE_COMBINED];
        for (i, a) in lanes.iter().enumerate() {
            assert_ne!(*a, SAVE_LANE_NONE);
            for b in &lanes[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn bypass_token_lifecycle() {
        // ONE serial test on purpose: the bypass statics are process-global and the
        // test harness runs tests concurrently; splitting these assertions across
        // tests would race. No other test touches ARMED or the bypass statics.
        ARMED.store(1, Ordering::SeqCst);
        assert!(!bypass_pending());

        // Arm; a second arm while pending is refused.
        assert!(arm_one_save_bypass());
        assert!(bypass_pending());
        assert!(!arm_one_save_bypass());
        assert_eq!(BYPASS_ARMED_TOTAL.load(Ordering::SeqCst), 1);
        // Arming re-sentinels the final status for the new commit cycle.
        assert_eq!(bypass_final_status_raw(), BYPASS_FINAL_STATUS_NONE);
        assert_eq!(take_bypass_final_status(), None);

        // Consume as the enqueue hook does; then expiring finds nothing pending.
        assert!(
            BYPASS_TOKEN
                .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        );
        assert!(!bypass_pending());
        assert!(!expire_bypass_if_pending());

        // Terminal-status handshake: fresh exactly once, latched value persists.
        BYPASS_FINAL_STATUS.store(0, Ordering::SeqCst);
        BYPASS_FINAL_STATUS_FRESH.store(1, Ordering::SeqCst);
        assert_eq!(take_bypass_final_status(), Some(0));
        assert_eq!(take_bypass_final_status(), None);
        assert_eq!(bypass_final_status_raw(), 0);

        // A stranded token is expired by the watchdog path.
        assert!(arm_one_save_bypass());
        assert!(expire_bypass_if_pending());
        assert!(!bypass_pending());
        assert_eq!(BYPASS_EXPIRED_TOTAL.load(Ordering::SeqCst), 1);

        // Disarmed suppression refuses to arm a token at all.
        ARMED.store(0, Ordering::SeqCst);
        assert!(!arm_one_save_bypass());
        assert_eq!(BYPASS_ARMED_TOTAL.load(Ordering::SeqCst), 2);
    }

    /// The submit builders' precondition, transcribed from the 1.16.2 decompile of
    /// `FUN_140e6ef60`: `iodev+0x10 == 0 && iodev+0x20 == 0`. Neither `+0x18` nor `+0x28`
    /// is an operand of it -- `+0x18` only tells us WHO owns a latched `+0x20`, and a
    /// non-zero `+0x28` makes the builder DEFER (return 1) rather than decline.
    #[test]
    fn a_save_is_admitted_by_exactly_the_two_guard_operands() {
        let clear = SlRequestSlot::default();
        assert!(clear.admits_a_save());

        assert!(
            SlRequestSlot {
                load_content: 0xdead,
                file_cap: 0xbeef,
                deferred_opcode: 7,
                ..clear
            }
            .admits_a_save(),
            "+0x18/+0x28/+0x30 are not operands of the submit precondition"
        );
        assert!(
            !SlRequestSlot {
                save_content: 0x1,
                ..clear
            }
            .admits_a_save()
        );
        assert!(!SlRequestSlot { job: 0x1, ..clear }.admits_a_save());
    }

    #[test]
    fn every_bail_classification_is_distinct_and_named() {
        let cases = [
            (None, SL_BAIL_IODEV_UNREADABLE, "iodev-unreadable"),
            (
                Some(SlRequestSlot::default()),
                SL_BAIL_PRECONDITION_CLEAR,
                "precondition-clear-builder-alloc-refused",
            ),
            (
                Some(SlRequestSlot {
                    save_content: 0x2000,
                    ..SlRequestSlot::default()
                }),
                SL_BAIL_SAVE_CONTENT_LATCHED,
                "save-content-latched-0x10",
            ),
            (
                Some(SlRequestSlot {
                    save_content: 0x2000,
                    job: 0x3000,
                    ..SlRequestSlot::default()
                }),
                SL_BAIL_SAVE_CONTENT_AND_JOB_LATCHED,
                "save-content-and-job-latched-0x10+0x20",
            ),
            (
                Some(SlRequestSlot {
                    load_content: 0x4000,
                    job: 0x3000,
                    ..SlRequestSlot::default()
                }),
                SL_BAIL_LOAD_JOB_LATCHED,
                "load-job-latched-0x18+0x20",
            ),
            (
                Some(SlRequestSlot {
                    job: 0x3000,
                    ..SlRequestSlot::default()
                }),
                SL_BAIL_JOB_LATCHED,
                "orphan-job-latched-0x20",
            ),
        ];
        let mut seen = Vec::new();
        for (slot, expected_code, expected_label) in cases {
            let code = classify_sl_bail(slot);
            assert_eq!(code, expected_code, "misclassified {slot:?}");
            assert_eq!(sl_bail_reason_label(code), expected_label);
            assert!(
                !bail_verdict(code).is_empty(),
                "every reachable code needs a verdict a reader can act on"
            );
            assert!(!seen.contains(&code), "reason codes must be distinct");
            seen.push(code);
        }
        // The sentinel must never be produced by a real sample, or "no decline yet" and
        // "a decline we could not explain" would read identically.
        assert!(!seen.contains(&SL_BAIL_UNSAMPLED));
        assert_eq!(sl_bail_reason_label(SL_BAIL_UNSAMPLED), "unsampled");
    }

    /// A classification is only useful if a reader can see the operand behind it, and the
    /// operands are the whole point: `+0x10` non-zero and `+0x20` non-zero are the same
    /// verdict from the guard's point of view but different bugs.
    #[test]
    fn the_slot_description_carries_every_operand_value() {
        let text = describe_slot(Some(SlRequestSlot {
            save_content: 0xaa,
            load_content: 0xbb,
            job: 0xcc,
            file_cap: 0xdd,
            deferred_opcode: 9,
        }));
        for needle in ["0xaa", "0xbb", "0xcc", "0xdd", "deferred_opcode=9"] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
        assert!(text.contains("save-content-and-job-latched-0x10+0x20"));
        assert_eq!(describe_slot(None), "iodev UNREADABLE");
    }

    /// `iodev+0x20` is shared, so "a load may be built" and "a save may be built" are the
    /// same question asked of different content fields. The load builders test `+0x18` and
    /// `+0x20`; `+0x10` is none of their business.
    #[test]
    fn a_load_is_admitted_by_exactly_the_two_load_operands() {
        let clear = SlRequestSlot::default();
        assert!(clear.admits_a_load());
        assert!(
            SlRequestSlot {
                save_content: 0xdead,
                file_cap: 0xbeef,
                deferred_opcode: 7,
                ..clear
            }
            .admits_a_load(),
            "+0x10/+0x28/+0x30 are not operands of the load builders' precondition"
        );
        assert!(
            !SlRequestSlot {
                load_content: 0x1,
                ..clear
            }
            .admits_a_load()
        );
        assert!(!SlRequestSlot { job: 0x1, ..clear }.admits_a_load());
    }

    /// The load-consumer oracle has to tell three things apart that look alike from a
    /// counter: nothing was owed, the debt was paid, and the debt is still outstanding.
    /// Only the third one refuses every later save.
    #[test]
    fn the_load_consumer_outcomes_are_distinct_and_named() {
        let clear = SlRequestSlot::default();
        let held = SlRequestSlot {
            load_content: 0x4000,
            job: 0x3000,
            ..clear
        };
        let cases = [
            (None, Some(clear), LOAD_CONSUMER_UNREADABLE),
            (Some(held), None, LOAD_CONSUMER_UNREADABLE),
            (Some(clear), Some(clear), LOAD_CONSUMER_NOTHING_HELD),
            (Some(held), Some(clear), LOAD_CONSUMER_RELEASED),
            (Some(held), Some(held), LOAD_CONSUMER_STILL_HELD),
        ];
        let mut seen = Vec::new();
        for (before, after, expected) in cases {
            let code = classify_load_consumer(before, after);
            assert_eq!(code, expected, "misclassified {before:?} -> {after:?}");
            assert!(!load_consumer_outcome_label(code).is_empty());
            if !seen.contains(&code) {
                seen.push(code);
            }
        }
        assert_eq!(seen.len(), 4, "each outcome needs its own code");
        assert!(!seen.contains(&LOAD_CONSUMER_UNSAMPLED));
        assert_eq!(
            load_consumer_outcome_label(LOAD_CONSUMER_UNSAMPLED),
            "unsampled"
        );
    }

    /// A save-side request left behind is a DIFFERENT bug from a load-side one, and the
    /// load-consumer oracle must not claim credit for clearing `+0x10`. Only `+0x18`/`+0x20`
    /// are its debt; a lingering `+0x10` still fails the save builders and is reported by
    /// `classify_sl_bail`, not by this one.
    #[test]
    fn the_load_consumer_only_speaks_for_the_load_side() {
        let before = SlRequestSlot {
            save_content: 0x1000,
            load_content: 0x4000,
            job: 0x3000,
            ..SlRequestSlot::default()
        };
        let after = SlRequestSlot {
            save_content: 0x1000,
            ..SlRequestSlot::default()
        };
        assert_eq!(
            classify_load_consumer(Some(before), Some(after)),
            LOAD_CONSUMER_RELEASED
        );
        assert_eq!(
            classify_sl_bail(Some(after)),
            SL_BAIL_SAVE_CONTENT_LATCHED,
            "the save side is still latched and must still be reported as such"
        );
    }
}
