// THE WRITE-COMPLETION EVENT for a bypassed save.
//
// `include!`d into `lib.rs` so it shares the crate's flat module namespace (same pattern
// the product's telemetry writers use) while keeping each file under the size gate.
//
// `FUN_14240fd70` == `SaveLoad2::SLSaveSession`'s job body, the function the SL worker
// thread runs to perform one save. Its RETURN is the moment the bypassed save has finished
// writing, and it is the ONLY completion signal in this chain that does not require
// somebody to poll.
//
// Why a poll cannot be relied on (1.16.2 decompile, shift 0):
//
//   * `FUN_140e6e430` (the status poll) reports "terminal" as case `0x14`, and 0x14 is
//     produced by `FUN_14240a1f0` ONLY when `FUN_14240c270(queue, id)` fails to find the
//     job -- i.e. after the worker has REMOVED it from the queue. It is strictly later than
//     the write, and nothing in the job body ever sets a terminal state itself: the body
//     sets `job+0x98` to 0x16 on entry and 2 mid-write, and BOTH map to the poll's
//     `break -> return 1` ("still in flight") arm.
//   * The poll has exactly two consumers: `CS::MoveMapStep::DoSaveStuff`, gated on
//     `GameMan::IsSaveState1()`, and `FUN_14082a0f0`, the "saving..." MenuJob. The Save Game
//     commit closes the menus BEFORE it fires (deliberately -- it is what stops the b73-only
//     lane stealing the bypass token), so the MenuJob consumer may not exist at all for our
//     commit.
//
// A measured switch-then-save commit confirmed the consequence: the file was rewritten in
// place and verified, and `bypass_final_status` was still unset 900 ticks later.
//
// The body's own result is right there when it returns. `SLSession` (base ctor
// `FUN_14240d110`) holds a refcounted `SaveLoad2::detail::SLSessionResultInfo*` at `+0xa0`;
// `FUN_14240ebd0` initialises its `+0x10` code to 0, every failure path in the body writes a
// non-zero code there through `FUN_14240dc40`, and `FUN_14240a180` -- the getter the poll
// itself uses on the terminal arm -- reads that exact field. Reading it at the body's return
// therefore yields the same verdict the poll would eventually report, one queue-drain
// earlier and with nobody having to ask.

/// `FUN_14240fd70` -- `SaveLoad2::SLSaveSession`'s job body (worker thread), reached only
/// through the vtable the save-job constructor `FUN_14240fa50` installs. That constructor
/// has ONE call site, `FUN_14240e6f0`, the save-side worker enqueue -- so this body runs
/// for saves and for nothing else. Loads build a different job through `FUN_14240e420`.
#[cfg(windows)]
const SL_SAVE_JOB_BODY_RVA: usize = 0x240fd70;
/// `mov rax,rsp; push rbp/rdi/r12/r14/r15; lea rbp,[rax-0x5f]; sub rsp,0xb0`.
const SL_SAVE_JOB_BODY_SIG: &[u8] = &[
    0x48, 0x8B, 0xC4, 0x55, 0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8D, 0x68, 0xA1, 0x48,
    0x81, 0xEC, 0xB0, 0x00, 0x00, 0x00,
];
/// `SLSession+0xa0` -- the refcounted `SLSessionResultInfo*` the base ctor allocates.
#[cfg(windows)]
const SL_JOB_RESULT_INFO_OFFSET: usize = 0xa0;
/// `SLSessionResultInfo+0x10` -- the job's result code. 0 = success (the ctor's value, left
/// untouched by a clean run); every failure path writes a non-zero code.
#[cfg(windows)]
const SL_JOB_RESULT_CODE_OFFSET: usize = 0x10;
/// Stand-in result for "the job body completed but its result object was unreadable".
/// [`save_job_result_to_status`] maps it to the generic failure status, never to success.
pub const SAVE_JOB_RESULT_UNREADABLE: u32 = 0xffff_ffff;
/// The poll's generic "the job finished and did not succeed" code, used when a bypassed save
/// dies at the submit and no native result object will ever exist.
const SL_STATUS_SUBMIT_FAILED: u32 = 9;

/// `SLSaveSession`'s job body: one argument, `this`, no return value.
#[cfg(windows)]
type SaveJobBodyFn = unsafe extern "system" fn(usize);

/// Which observation produced the latched `BYPASS_FINAL_STATUS`. A commit that had to wait
/// for the product-side watchdog leaves this at [`BYPASS_STATUS_SOURCE_NONE`], so "the save
/// completed" and "we never found out" can never read the same.
static BYPASS_FINAL_STATUS_SOURCE: AtomicUsize = AtomicUsize::new(BYPASS_STATUS_SOURCE_NONE);
/// Commits whose terminal status came from a native poll consumer (the pre-existing path:
/// `DoSaveStuff` or the "saving..." MenuJob happened to be polling).
static BYPASS_COMPLETED_VIA_POLL_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Commits whose terminal status came from the SL save-job body returning on the worker
/// thread -- the path that does not need anybody to poll.
static BYPASS_COMPLETED_VIA_JOB_TOTAL: AtomicU64 = AtomicU64::new(0);

/// No terminal status has been observed for the current commit.
pub const BYPASS_STATUS_SOURCE_NONE: usize = 0;
/// The status came from a native poll consumer calling `FUN_140e6e430`.
pub const BYPASS_STATUS_SOURCE_POLL: usize = 1;
/// The status came from `SLSaveSession`'s job body returning on the SL worker thread.
pub const BYPASS_STATUS_SOURCE_JOB: usize = 2;
/// The native enqueue itself reported failure, so no job will ever run. Distinct from
/// [`BYPASS_STATUS_SOURCE_NONE`]: the save is known-dead rather than merely unobserved.
pub const BYPASS_STATUS_SOURCE_SUBMIT_FAILED: usize = 3;

/// Trampoline for `FUN_14240fd70` (0 = the observer is not installed).
#[cfg(windows)]
static ORIG_SAVE_JOB_BODY: AtomicUsize = AtomicUsize::new(0);
/// Times the SL worker picked up a save job and entered its body. Under armed suppression
/// the only enqueue that ever reaches the worker is a bypassed one, so this is "times a
/// sanctioned save actually started writing".
static SAVE_JOB_STARTS: AtomicU64 = AtomicU64::new(0);
/// Times a save job body RETURNED. This is the write-completion event.
static SAVE_JOB_COMPLETIONS: AtomicU64 = AtomicU64::new(0);
/// `SLSessionResultInfo+0x10` read at the last body return, or
/// [`SAVE_JOB_RESULT_UNREADABLE`].
static SAVE_JOB_LAST_RESULT: AtomicU32 = AtomicU32::new(SAVE_JOB_RESULT_UNREADABLE);
/// [`SAVE_JOB_COMPLETIONS`] sampled immediately BEFORE the bypassed enqueue was forwarded.
/// The adoption below requires a completion STRICTLY AFTER this, so a job that finished
/// before our submit can never be mistaken for ours.
static SAVE_JOB_COMPLETIONS_AT_ALLOW: AtomicU64 = AtomicU64::new(0);
/// 1 once the job-body observer is bound. Zero means every commit must fall back to the
/// product-side watchdog, which is a degraded run and has to be visible as one.
static SAVE_JOB_OBSERVER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Body entries that found no trampoline. Structurally unreachable (the trampoline is stored
/// before the hook is enabled) and counted rather than assumed away, because on this path the
/// user's save would not be written at all.
static SAVE_JOB_NO_TRAMPOLINE: AtomicU64 = AtomicU64::new(0);

/// Human label for [`bypass_final_status_source`].
pub fn bypass_final_status_source_label(source: usize) -> &'static str {
    match source {
        BYPASS_STATUS_SOURCE_POLL => "native-poll",
        BYPASS_STATUS_SOURCE_JOB => "worker-job-completion",
        BYPASS_STATUS_SOURCE_SUBMIT_FAILED => "native-enqueue-failed",
        _ => "none",
    }
}

/// Which observation produced the latched terminal status (`BYPASS_STATUS_SOURCE_*`).
pub fn bypass_final_status_source() -> usize {
    BYPASS_FINAL_STATUS_SOURCE.load(Ordering::SeqCst)
}

/// Commits completed on a native poll consumer's answer.
pub fn bypass_completed_via_poll_total() -> u64 {
    BYPASS_COMPLETED_VIA_POLL_TOTAL.load(Ordering::SeqCst)
}

/// Commits completed on the SL worker's job-body return.
pub fn bypass_completed_via_job_total() -> u64 {
    BYPASS_COMPLETED_VIA_JOB_TOTAL.load(Ordering::SeqCst)
}

/// Times the SL worker entered a save job body (a sanctioned save started writing).
pub fn save_job_starts() -> u64 {
    SAVE_JOB_STARTS.load(Ordering::SeqCst)
}

/// Times a save job body returned (a save finished writing).
pub fn save_job_completions() -> u64 {
    SAVE_JOB_COMPLETIONS.load(Ordering::SeqCst)
}

/// True when no SL save job body is executing at this instant.
///
/// The read order is load-bearing. The observer increments `STARTS` before the body and
/// `COMPLETIONS` after it, so `starts >= completions` always. Sampling COMPLETIONS first and
/// STARTS second means that if the two are equal, they were equal at the later of the two
/// reads: `completions` can only have grown since its read, and it is bounded above by
/// `starts`, which we read afterwards. Sampling in the other order admits a job that started
/// between the reads and reports it as quiescent.
///
/// This is the writer-side interlock the destination redirect needs. The native in-place
/// writer `FUN_1424142e0` opens the container ONCE PER DIRTY BLOCK, so a redirect window that
/// closes mid-body patches the remaining blocks into whatever the unredirected path resolves
/// to -- the very file the user chose not to overwrite.
pub fn save_job_writer_idle() -> bool {
    let completions = SAVE_JOB_COMPLETIONS.load(Ordering::SeqCst);
    let starts = SAVE_JOB_STARTS.load(Ordering::SeqCst);
    starts == completions
}

/// The result code read out of the last completed job, or [`SAVE_JOB_RESULT_UNREADABLE`].
pub fn save_job_last_result() -> u32 {
    SAVE_JOB_LAST_RESULT.load(Ordering::SeqCst)
}

/// Whether the job-body completion observer is bound. False means completion detection is
/// back to "whatever happens to poll", i.e. the watchdog.
pub fn save_job_observer_installed() -> bool {
    SAVE_JOB_OBSERVER_INSTALLED.load(Ordering::SeqCst) != 0
}

/// Body entries that found no trampoline (see [`SAVE_JOB_NO_TRAMPOLINE`]).
pub fn save_job_no_trampoline() -> u64 {
    SAVE_JOB_NO_TRAMPOLINE.load(Ordering::SeqCst)
}

/// Map an `SLSessionResultInfo` code onto the status space [`take_bypass_final_status`]
/// reports in, using the game's own table.
///
/// This is `FUN_140e6e430`'s terminal (`case 0x14`) arm verbatim: it calls `FUN_14240a180`,
/// which returns exactly this field, and then maps `0 -> 0`, `3 -> 7`, `2|4 -> 8`, `7 -> 2`,
/// everything else `-> 9`. Reproducing the mapping rather than the SIDE EFFECTS is
/// deliberate -- the native arm also releases the request and sets the `DAT_14458937d`
/// save-error flag, and an observer must not do either.
///
/// Note what this cannot do: turn an unknown code into a success. Only the literal 0 the
/// constructor writes and no failure path overwrites maps to 0.
pub fn save_job_result_to_status(result: u32) -> u32 {
    match result {
        0 => SL_STATUS_SUCCESS,
        3 => 7,
        2 | 4 => 8,
        7 => 2,
        _ => 9,
    }
}

/// Latch the current commit as FAILED because the submit itself did not happen.
///
/// Uses the same one-winner CAS as the two success paths, so a failure can never overwrite
/// an outcome that was already observed. The status is the poll's generic "finished, not
/// successful" code -- there is no native result object to read, because no job exists.
#[cfg(windows)]
fn latch_submit_failure_as_final_status(why: &str) {
    if BYPASS_COMPLETION_WATCH
        .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    BYPASS_FINAL_STATUS.store(SL_STATUS_SUBMIT_FAILED, Ordering::SeqCst);
    BYPASS_FINAL_STATUS_SOURCE.store(BYPASS_STATUS_SOURCE_SUBMIT_FAILED, Ordering::SeqCst);
    BYPASS_FINAL_STATUS_FRESH.store(1, Ordering::SeqCst);
    log_message(format_args!(
        "suppress: bypassed save is DEAD ON SUBMIT ({why}) -- terminal status \
         {SL_STATUS_SUBMIT_FAILED} latched now; no job was queued, so nothing will ever \
         report on it"
    ));
}

/// Record the worker's completion counter at the instant a bypassed submit is forwarded.
///
/// Sampled BEFORE the native enqueue call, not after: the worker can pick the job up and
/// finish it before the enqueue even returns, and a later sample would then be unable to
/// tell our own completion from a pre-existing one.
#[cfg(windows)]
fn arm_save_job_completion_watch() {
    SAVE_JOB_COMPLETIONS_AT_ALLOW.store(SAVE_JOB_COMPLETIONS.load(Ordering::SeqCst), Ordering::SeqCst);
}

/// Adopt a completed SL save job as the current commit's terminal status.
///
/// Called from the product's commit tick. Does nothing unless ALL of these hold, which is
/// what makes it positive evidence rather than an absence of failure:
///
///   1. a completion watch is live -- our one-shot token was CONSUMED by an enqueue, so a
///      real submit happened and no terminal status has been latched for it yet;
///   2. [`SAVE_JOB_COMPLETIONS`] has advanced past the value sampled at that enqueue, so the
///      job that finished is one that started after our submit;
///   3. the CAS on the watch succeeds, so exactly one of this and the poll detour can ever
///      latch a given commit's outcome.
///
/// Returns the mapped status on the tick it adopts one, and `None` on every other tick.
pub fn adopt_completed_save_job_as_final_status() -> Option<u32> {
    if BYPASS_COMPLETION_WATCH.load(Ordering::SeqCst) == 0 {
        return None;
    }
    let completions = SAVE_JOB_COMPLETIONS.load(Ordering::SeqCst);
    if completions <= SAVE_JOB_COMPLETIONS_AT_ALLOW.load(Ordering::SeqCst) {
        return None;
    }
    if BYPASS_COMPLETION_WATCH
        .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // A poll consumer answered in the same frame. Its value is the same field read
        // through the game's own getter, so there is nothing to reconcile -- leave it.
        return None;
    }
    let result = SAVE_JOB_LAST_RESULT.load(Ordering::SeqCst);
    let status = save_job_result_to_status(result);
    BYPASS_FINAL_STATUS.store(status, Ordering::SeqCst);
    BYPASS_FINAL_STATUS_SOURCE.store(BYPASS_STATUS_SOURCE_JOB, Ordering::SeqCst);
    BYPASS_FINAL_STATUS_FRESH.store(1, Ordering::SeqCst);
    let count = BYPASS_COMPLETED_VIA_JOB_TOTAL.fetch_add(1, Ordering::SeqCst) + 1;
    log_message(format_args!(
        "suppress: bypassed save WROTE THROUGH -- SLSaveSession job body returned with \
         result={result} (0=success) -> terminal status={status} (#{count} completed on the \
         worker-thread signal); no poll consumer was needed"
    ));
    publish_snapshot();
    Some(status)
}

/// Observer on `FUN_14240fd70`, `SaveLoad2::SLSaveSession`'s job body. **Runs on the SL
/// worker thread**, not the game thread.
///
/// Pure observation: it calls the body and records two atomics around it. It deliberately
/// does NOT log and does NOT publish. The body is the code that opens the save file, copies
/// `ER0000.sl2.bak` and writes every block, and this host DLL detours file APIs of its own
/// -- emitting a log line or serializing a telemetry snapshot from inside that call would
/// re-enter those detours on the writer's own thread. The counters are read (and the human
/// line emitted) by the product's commit tick on the game thread instead.
///
/// The result is read AFTER the body returns, from `SLSession+0xa0 -> +0x10`, which is the
/// same field `FUN_14240a180` returns and the poll's terminal arm maps. The job object is
/// still owned by the queue at that point, so the read is against live memory.
#[cfg(windows)]
unsafe extern "system" fn save_job_body_hook(job: usize) {
    SAVE_JOB_STARTS.fetch_add(1, Ordering::SeqCst);
    let orig = ORIG_SAVE_JOB_BODY.load(Ordering::SeqCst);
    if orig == 0 {
        // Unreachable: the trampoline is stored before the hook is enabled, so nothing can
        // enter this detour first. Counted anyway -- on this path the write would simply not
        // happen, and a silent no-op is the worst possible way to lose a user's save.
        SAVE_JOB_NO_TRAMPOLINE.fetch_add(1, Ordering::SeqCst);
        return;
    }
    let original: SaveJobBodyFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(job) };
    let result = read_save_job_result(job).unwrap_or(SAVE_JOB_RESULT_UNREADABLE);
    SAVE_JOB_LAST_RESULT.store(result, Ordering::SeqCst);
    // LAST, and after the result store: the game-thread adopter gates on this counter, so it
    // must not become visible before the value it certifies.
    SAVE_JOB_COMPLETIONS.fetch_add(1, Ordering::SeqCst);
}

/// Read `SLSession+0xa0 -> SLSessionResultInfo+0x10` for a job that has just finished.
#[cfg(windows)]
fn read_save_job_result(job: usize) -> Option<u32> {
    use er_game_base::mem::{safe_read_i32, safe_read_usize};

    const MIN_PLAUSIBLE_POINTER: usize = 0x10000;

    if job < MIN_PLAUSIBLE_POINTER {
        return None;
    }
    let info = unsafe { safe_read_usize(job + SL_JOB_RESULT_INFO_OFFSET) }?;
    if info < MIN_PLAUSIBLE_POINTER {
        return None;
    }
    unsafe { safe_read_i32(info + SL_JOB_RESULT_CODE_OFFSET) }.map(|code| code as u32)
}

/// Bind the write-completion observer. Its own `MhHook`: nothing else in the process detours
/// `FUN_14240fd70`, and it is a one-argument void target the union's four-argument shape does
/// not describe.
///
/// Non-fatal like every other observer -- losing it costs the prompt completion signal and
/// falls the commit back on the product-side watchdog, which is a degraded outcome the
/// telemetry names rather than a broken save path.
#[cfg(windows)]
fn install_save_job_body_observer() -> bool {
    let bound = match verify(
        SL_SAVE_JOB_BODY_RVA,
        SL_SAVE_JOB_BODY_SIG,
        "SLSaveSessionJobBody",
    ) {
        Some(address) => {
            match unsafe { MhHook::new(address as *mut c_void, save_job_body_hook as *mut c_void) } {
                Ok(hook) => {
                    ORIG_SAVE_JOB_BODY.store(hook.trampoline() as usize, Ordering::SeqCst);
                    match unsafe { hook.queue_enable() } {
                        Ok(()) => match unsafe { MH_ApplyQueued() } {
                            MH_STATUS::MH_OK => true,
                            status => {
                                ORIG_SAVE_JOB_BODY.store(0, Ordering::SeqCst);
                                log_message(format_args!(
                                    "suppress: save-job-body observer MH_ApplyQueued failed: \
                                     {status:?} -- suppression stays armed, but a bypassed \
                                     save's completion can only be seen by a poll consumer"
                                ));
                                false
                            }
                        },
                        Err(status) => {
                            ORIG_SAVE_JOB_BODY.store(0, Ordering::SeqCst);
                            log_message(format_args!(
                                "suppress: save-job-body observer queue_enable failed: \
                                 {status:?} -- suppression stays armed, but a bypassed save's \
                                 completion can only be seen by a poll consumer"
                            ));
                            false
                        }
                    }
                }
                Err(status) => {
                    log_message(format_args!(
                        "suppress: save-job-body observer MhHook::new @0x{address:x} failed: \
                         {status:?} -- suppression stays armed, but a bypassed save's \
                         completion can only be seen by a poll consumer"
                    ));
                    false
                }
            }
        }
        None => false,
    };
    SAVE_JOB_OBSERVER_INSTALLED.store(usize::from(bound), Ordering::SeqCst);
    bound
}
