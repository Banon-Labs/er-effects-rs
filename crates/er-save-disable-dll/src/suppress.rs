//! Save-write suppression: swallow the SL submit, report the save as succeeded.
//!
//! # What this does, in one sentence
//!
//! It stops ELDEN RING from ever *enqueueing* a save-write job, and then answers the
//! game's own "did my save finish?" question with the code that means SUCCESS -- so no
//! byte is ever written, no backup is copied or deleted, and every native observer of
//! the save lifecycle sees exactly the state a real successful save leaves behind.
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

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[cfg(windows)]
use std::ffi::c_void;

#[cfg(windows)]
use er_game_base::mem::{game_rva, read_bytes};
#[cfg(windows)]
use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

#[cfg(windows)]
use crate::log_message;

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
const SL_RELEASE_REQUEST_RVA: usize = 0xe6f200;

/// The status code `FUN_140e6e430` returns when `iodev+0x10 == 0`, meaning "there is
/// no save request". Its single producer is the `MOV EAX,0x4` at `0x140e6e460`.
const SL_STATUS_NO_REQUEST: u32 = 4;
/// The status code that means "the save completed successfully". `DoSaveStuff` maps it
/// to the only arm that advances `GameMan+0xbc4` 2 -> 3.
const SL_STATUS_SUCCESS: u32 = 0;

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

/// The two detours that actually suppress: the submit swallow and the status rewrite.
/// The quit-settle observer is deliberately NOT one of them -- it changes nothing and a
/// failure to install it must not read as a partial suppression.
pub(crate) const SUPPRESSOR_HOOKS: usize = 2;

static ARMED: AtomicUsize = AtomicUsize::new(0);
static INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SETTLE_OBSERVER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static PROLOGUE_MISMATCHES: AtomicUsize = AtomicUsize::new(0);

static SUBMITS_SWALLOWED: AtomicU64 = AtomicU64::new(0);
static SUBMITS_PASSED_THROUGH: AtomicU64 = AtomicU64::new(0);
static STATUS_FAKED: AtomicU64 = AtomicU64::new(0);
static STATUS_PASSED_THROUGH: AtomicU64 = AtomicU64::new(0);
static RELEASE_UNAVAILABLE: AtomicU64 = AtomicU64::new(0);

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
pub(crate) fn installed_hooks() -> usize {
    INSTALLED.load(Ordering::SeqCst)
}

pub(crate) fn is_armed() -> bool {
    ARMED.load(Ordering::SeqCst) != 0
}

/// Whether the quit-settle observer bound. When false, `quit_phase_settle_events` can
/// only ever be 0, and a harness must not read that as a deadlock.
pub(crate) fn settle_observer_installed() -> bool {
    SETTLE_OBSERVER_INSTALLED.load(Ordering::SeqCst) != 0
}

pub(crate) fn counters() -> [(&'static str, u64); 8] {
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
    ]
}

/// Decide what a poll should report.
///
/// Split out as a pure function so the one rule that matters -- *only ever rewrite the
/// "no request" code, and only after we have actually swallowed something* -- is unit
/// testable on the host, with no game and no hooking involved.
pub(crate) fn decide_status(raw: u32, armed: bool, swallowed: u64) -> u32 {
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
pub(crate) fn prologue_matches(actual: &[u8], expected: &[u8]) -> bool {
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
pub(crate) fn should_report(count: u64, novel: bool) -> bool {
    novel || count.is_power_of_two()
}

/// Opcodes already seen at the choke point, as a bitmask.
///
/// A save opcode never seen before means a different *kind* of save funnelled through,
/// which is exactly the sort of thing this DLL exists to discover -- so it is reported
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
/// All-or-nothing on purpose. Binding only the submit detour would leave every save
/// stuck reporting "no request" and hang System->Quit on the `bc4 == 3` wait; binding
/// only the status detour would rewrite statuses for saves that really happened. A
/// partial install is worse than none, so a failure of either backs the whole thing out.
#[cfg(windows)]
pub(crate) fn install() -> usize {
    if crate::census_only_requested() {
        log_message(format_args!(
            "suppress: DISARMED by {} -- census-only positive-control run; saves will be \
             written normally and path diversion is off too, so the census must observe \
             them",
            crate::CENSUS_ONLY_ENV
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
    // Appended separately: it is an OBSERVER, and unlike the two suppressors a failure
    // to install it must not abort the install. Losing the deadlock counter costs
    // evidence; losing a suppressor would hang System->Quit.
    let mut targets: Vec<(&str, usize, *mut c_void, &AtomicUsize)> = targets.to_vec();
    if let Some(settle) = settle {
        targets.push((
            "QuitPhaseSettle",
            settle,
            quit_phase_settle_hook as *mut c_void,
            &ORIG_QUIT_PHASE_SETTLE,
        ));
    } else {
        log_message(format_args!(
            "suppress: quit-settle observer NOT installed -- suppression still active, but \
             this run cannot prove the quit path was released"
        ));
    }

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
            // Count the SUPPRESSORS only. Folding the optional observer into this total
            // made a healthy install report "suppression hooks=3/2", which reads like a
            // broken invariant on a run where everything worked. The observer is a
            // separate, independently-optional fact and is reported as one.
            INSTALLED.store(SUPPRESSOR_HOOKS, Ordering::SeqCst);
            SETTLE_OBSERVER_INSTALLED.store(usize::from(settle.is_some()), Ordering::SeqCst);
            ARMED.store(1, Ordering::SeqCst);
            log_message(format_args!(
                "suppress: ARMED -- SL_EnqueueSaveJob @0x{enqueue:x}, \
                 SL_PollSaveStatus @0x{poll:x}, SL_ReleaseRequest @0x{release:x}; \
                 quit-settle observer={}; no save write job will be enqueued and every \
                 save will report success",
                if settle.is_some() { "yes" } else { "NO" }
            ));
            // The SUPPRESSOR count, not `hooks.len()`. Returning the vector length folded
            // the optional observer in and made a healthy install log "3/2".
            SUPPRESSOR_HOOKS
        }
        status => {
            log_message(format_args!("suppress: MH_ApplyQueued failed: {status:?}"));
            0
        }
    }
}

/// Detour on `FUN_140e6fb50`.
///
/// The caller has already allocated an `SLSaveContent` into `iodev+0x10` and filled it
/// with the serialized blocks. We do not enqueue it. We hand it straight to the game's
/// own teardown -- the exact call the native code makes when the enqueue fails -- and
/// then report success, which is the one thing the native failure path does not do.
///
/// Releasing through `FUN_140e6f200` is not optional: leaving `iodev+0x10` populated
/// would permanently fail the `iodev+0x10 == 0 && iodev+0x20 == 0` precondition on
/// every later submit, and would leave the status poll dereferencing a null job.
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

    let released: ReleaseRequestFn = unsafe { core::mem::transmute(release) };
    unsafe { released(iodev) };

    let count = SUBMITS_SWALLOWED.fetch_add(1, Ordering::SeqCst) + 1;
    // Swallowing is the EXPECTED steady state, not an event. It is counted in telemetry
    // (`suppress_submits_swallowed`), which is what a harness reads; the log only needs
    // to show that it started, that it kept happening, and any new kind of save.
    let novel = note_opcode(opcode);
    if should_report(count, novel) {
        log_message(format_args!(
            "suppress: swallowed save submit #{count} (iodev=0x{iodev:x}, opcode={opcode}) \
             -- no job enqueued, request released, reporting submitted"
        ));
        // Publish on the same schedule. A snapshot per swallow meant a full JSON
        // re-serialize, `fs::write` and `fs::rename` on the GAME thread for every save
        // request -- this detours `FUN_140e6fb50`, whose callers are the per-frame
        // dispatchers, strictly above the `FUN_14240ae10` worker boundary -- and each of
        // those re-enters this DLL's own CreateFileW/MoveFileW detours. Every counter a
        // harness gates on is a threshold, and a threshold is crossed on the first
        // occurrence, which is always published.
        publish_snapshot();
    }
    1
}

/// Write the telemetry snapshot from a caller that is NOT already inside the census
/// reentrancy guard.
///
/// The suppression hooks are not observation paths, so they enter with the guard clear;
/// taking it here keeps `telemetry::write_snapshot`'s documented invariant -- that its
/// own file I/O cannot recurse into the census -- true for every caller rather than
/// true for some and merely-harmless for the rest.
#[cfg(windows)]
fn publish_snapshot() {
    let _ = crate::witness::with_guard(crate::telemetry::write_snapshot);
}

/// Detour on `FUN_140e6e430`.
///
/// Always runs the original first. The original's answer is the guard: only the literal
/// "no request" code is rewritten, and that code is produced by exactly one branch,
/// the `iodev+0x10 == 0` early-out. Any genuinely outstanding IO -- a save we did not
/// swallow, or a load, which lives in `iodev+0x18` -- cannot produce it, so it cannot
/// be lied about.
#[cfg(windows)]
unsafe extern "system" fn poll_save_status_hook(iodev: usize) -> u32 {
    let orig = ORIG_POLL_SAVE_STATUS.load(Ordering::SeqCst);
    if orig == 0 {
        return SL_STATUS_NO_REQUEST;
    }
    let original: PollSaveStatusFn = unsafe { core::mem::transmute(orig) };
    let raw = unsafe { original(iodev) };

    let decided = decide_status(raw, is_armed(), SUBMITS_SWALLOWED.load(Ordering::SeqCst));
    if decided == raw {
        STATUS_PASSED_THROUGH.fetch_add(1, Ordering::SeqCst);
    } else {
        STATUS_FAKED.fetch_add(1, Ordering::SeqCst);
    }
    sample_quit_phase();
    decided
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
        // report, and the DLL must be inert.
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
        // choke point -- exactly what this DLL exists to discover. Throttling must
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
    }
}
