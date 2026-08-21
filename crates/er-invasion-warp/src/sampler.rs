//! The driver that turns one fault-closed read into ORACLE 1.
//!
//! # Why a sampler and not a single read
//!
//! `CS::InGameStayStep::STEP_InGameStayLoad` (`0x140ae9860`) requests BOTH
//! `other:/AutoInvadePoint.aipbnd` and `_dlc02` at boot, and each is parsed asynchronously by
//! `AutoInvadePointBndFileCap::Process` (`0x140201660`) one `AddForBlockId` at a time. So a
//! read taken at an arbitrary tick can legitimately see: nothing (`CatalogEmpty`), a partly
//! built tree, base only, or base + dlc02. Reading once and reporting whatever came back is
//! exactly how oracle 1 would report 257 blocks on a DLC install and call it a MATCH.
//!
//! [`CatalogSampler`] therefore keeps sampling until the totals stop moving:
//!
//! * it latches IMMEDIATELY on [`crate::oracles::EXPECTED_CATALOG_BASE_DLC02`], the maximal
//!   state -- nothing can be added after it, so waiting longer proves nothing;
//! * otherwise it latches once the same totals come back
//!   [`CATALOG_STABLE_SAMPLES_TO_LATCH`] times running, which is how a base-only install (no
//!   DLC container to wait for) and a genuinely wrong reading both terminate;
//! * it gives up after [`CATALOG_SAMPLE_ATTEMPT_LIMIT`] attempts so a broken boot cannot keep
//!   the walk running on the game thread forever.
//!
//! The state machine is deliberately platform-independent and takes the read RESULT as its
//! input, so every path above is exercised by `cargo test` on the host with no game running.

use crate::invasion_warp::InvasionWarpCatalogSummary;
use crate::oracles::{CatalogFingerprintVerdict, classify_catalog};

/// Game-task ticks between samples. The walk is ~1100 fault-tolerant reads on the shipped
/// data, so it runs about once a second rather than every frame -- and stops entirely once the
/// sampler latches, which on a healthy boot is within a few seconds of the containers mounting.
pub const CATALOG_SAMPLE_INTERVAL_TICKS: u64 = 60;

/// Consecutive identical readings that latch a NON-maximal total. Long enough that a second
/// container still being parsed cannot be mistaken for a finished catalog.
pub const CATALOG_STABLE_SAMPLES_TO_LATCH: u32 = 8;

/// Attempts before the sampler stops trying. At [`CATALOG_SAMPLE_INTERVAL_TICKS`] this is
/// ~20 minutes of a 60 fps boot; past that the container is never coming.
pub const CATALOG_SAMPLE_ATTEMPT_LIMIT: u32 = 1200;

/// How many consecutive not-ready attempts get logged before the sampler goes quiet. Enough to
/// diagnose a stuck boot, few enough that a slow load does not fill the log.
pub const CATALOG_RETRY_LOG_LIMIT: u32 = 5;

/// What the sampler decided after one read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogSampleStep {
    /// Already finished; the driver must not read again.
    Finished,
    /// The singleton was not readable yet. `log` marks the attempts worth a log line.
    Retry { attempt: u32, log: bool },
    /// A catalog came back, but the totals may still grow. Publish and keep watching.
    Sampled {
        summary: InvasionWarpCatalogSummary,
        stable_streak: u32,
    },
    /// Final answer: the totals are settled. Publish, log the verdict, stop reading.
    Latched {
        summary: InvasionWarpCatalogSummary,
        verdict: CatalogFingerprintVerdict,
    },
    /// The attempt limit ran out with no readable catalog. Oracle 1 is UNPROVEN, not passed.
    GaveUp { attempts: u32 },
}

/// The latching state machine behind oracle 1.
#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogSampler {
    last: Option<InvasionWarpCatalogSummary>,
    stable_streak: u32,
    attempts: u32,
    failed_attempts: u32,
    finished: bool,
}

impl CatalogSampler {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last: None,
            stable_streak: 0,
            attempts: 0,
            failed_attempts: 0,
            finished: false,
        }
    }

    /// True once the sampler has latched or given up. The driver checks this BEFORE reading, so
    /// a settled catalog costs the game thread nothing at all.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// The totals most recently read, if any.
    #[must_use]
    pub const fn last_summary(&self) -> Option<InvasionWarpCatalogSummary> {
        self.last
    }

    /// Feed one read. `None` means the singleton was not readable/ready this time; the caller
    /// owns the reason string, so this stays a pure state machine.
    pub fn observe(&mut self, sample: Option<InvasionWarpCatalogSummary>) -> CatalogSampleStep {
        if self.finished {
            return CatalogSampleStep::Finished;
        }
        self.attempts += 1;
        let Some(summary) = sample else {
            self.failed_attempts += 1;
            if self.attempts >= CATALOG_SAMPLE_ATTEMPT_LIMIT {
                self.finished = true;
                return CatalogSampleStep::GaveUp {
                    attempts: self.attempts,
                };
            }
            return CatalogSampleStep::Retry {
                attempt: self.failed_attempts,
                log: self.failed_attempts <= CATALOG_RETRY_LOG_LIMIT,
            };
        };

        if self.last == Some(summary) {
            self.stable_streak += 1;
        } else {
            self.stable_streak = 1;
        }
        self.last = Some(summary);

        let verdict = classify_catalog(summary);
        let maximal = verdict == CatalogFingerprintVerdict::MatchBaseAndDlc02;
        if maximal || self.stable_streak >= CATALOG_STABLE_SAMPLES_TO_LATCH {
            self.finished = true;
            return CatalogSampleStep::Latched { summary, verdict };
        }
        if self.attempts >= CATALOG_SAMPLE_ATTEMPT_LIMIT {
            self.finished = true;
            return CatalogSampleStep::GaveUp {
                attempts: self.attempts,
            };
        }
        CatalogSampleStep::Sampled {
            summary,
            stable_streak: self.stable_streak,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Windows: the game-task tick the DLL registers.
// ---------------------------------------------------------------------------------------

/// Run one tick of the oracle-1 driver from the game's recurring task.
///
/// Cheap by construction: it returns before touching the engine when the host gate is off, when
/// this is not a sample tick, or once the sampler has settled. Everything it does read goes
/// through the fault-tolerant [`crate::live_read`] walk.
///
/// # Safety
///
/// Must be called from the game's task thread, after the FromSoftware runtime singletons exist
/// (`CSTaskImp` having resolved is that guarantee). Read-only: no writes into the engine, no
/// session/multiplayer side effects.
#[cfg(windows)]
pub unsafe fn invasion_warp_catalog_tick() {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TICKS: AtomicU64 = AtomicU64::new(0);
    static SAMPLER: Mutex<CatalogSampler> = Mutex::new(CatalogSampler::new());

    if !crate::host::invasion_warp_enabled() {
        return;
    }
    let tick = TICKS.fetch_add(1, Ordering::SeqCst);
    if !tick.is_multiple_of(CATALOG_SAMPLE_INTERVAL_TICKS) {
        return;
    }
    // The task runs on one thread, so this lock is uncontended; recovering from poisoning
    // keeps a panic in one tick from silencing the oracle for the rest of the run.
    let mut sampler = match SAMPLER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if sampler.is_finished() {
        return;
    }

    // SAFETY: forwarded from this function's own contract -- the caller guarantees the game
    // task context. The read itself is fault-closed and never dereferences on hope.
    let read = unsafe { crate::invasion_warp::collect_invasion_warp_catalog() };
    let (sample, reason) = match &read {
        Ok(catalog) => (Some(catalog.summary()), String::new()),
        Err(error) => (None, error.to_string()),
    };
    let step = sampler.observe(sample);
    drop(sampler);
    report_step(step, &reason, read.ok().as_ref());
}

/// Emit one sampler step: publish the counters, write the telemetry document, and log the
/// lines a user diagnoses the run from.
#[cfg(windows)]
fn report_step(
    step: CatalogSampleStep,
    reason: &str,
    catalog: Option<&crate::invasion_warp::InvasionWarpCatalog>,
) {
    use crate::host::append_autoload_debug;
    use crate::oracles::{
        NEGATIVE_ORACLES_UNMEASURED_NOTE, describe_catalog_oracle, publish_catalog_oracles,
    };

    match step {
        CatalogSampleStep::Finished => {}
        CatalogSampleStep::Retry { attempt, log } => {
            if log {
                append_autoload_debug(format_args!(
                    "invasion-warp oracle: catalog not ready (not-ready attempt {attempt}): {reason}"
                ));
                crate::oracles::publish_document("waiting", reason);
            }
        }
        CatalogSampleStep::Sampled {
            summary,
            stable_streak,
        } => {
            publish_catalog_oracles(summary);
            append_autoload_debug(format_args!(
                "invasion-warp oracle: {} [unsettled, same totals {stable_streak}/{CATALOG_STABLE_SAMPLES_TO_LATCH} samples]",
                describe_catalog_oracle(summary)
            ));
            crate::oracles::publish_document(
                "sampling",
                &format!("same totals for {stable_streak} consecutive samples"),
            );
        }
        CatalogSampleStep::Latched { summary, verdict } => {
            publish_catalog_oracles(summary);
            if let Some(catalog) = catalog {
                crate::invasion_warp::log_catalog_summary(catalog);
            }
            append_autoload_debug(format_args!(
                "invasion-warp oracle: {}",
                describe_catalog_oracle(summary)
            ));
            append_autoload_debug(format_args!(
                "invasion-warp oracle: {NEGATIVE_ORACLES_UNMEASURED_NOTE}"
            ));
            crate::oracles::publish_document(
                "latched",
                &format!("totals settled; verdict {}", verdict.tag()),
            );
        }
        CatalogSampleStep::GaveUp { attempts } => {
            append_autoload_debug(format_args!(
                "invasion-warp oracle: GAVE UP after {attempts} attempts -- oracle 1 is UNPROVEN, \
                 not passed. Last reason: {reason}"
            ));
            crate::oracles::publish_document(
                "gave_up",
                &format!("gave up after {attempts} attempts: {reason}"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracles::{EXPECTED_CATALOG_BASE, EXPECTED_CATALOG_BASE_DLC02};

    fn summary(blocks: usize, targets: usize, areas: usize) -> InvasionWarpCatalogSummary {
        InvasionWarpCatalogSummary {
            block_count: blocks,
            target_count: targets,
            area_count: areas,
        }
    }

    #[test]
    fn a_maximal_reading_latches_on_the_very_first_sample() {
        let mut sampler = CatalogSampler::new();
        assert_eq!(
            sampler.observe(Some(EXPECTED_CATALOG_BASE_DLC02)),
            CatalogSampleStep::Latched {
                summary: EXPECTED_CATALOG_BASE_DLC02,
                verdict: CatalogFingerprintVerdict::MatchBaseAndDlc02,
            }
        );
        assert!(sampler.is_finished());
        // And it never reads again.
        assert_eq!(
            sampler.observe(Some(EXPECTED_CATALOG_BASE)),
            CatalogSampleStep::Finished
        );
    }

    #[test]
    fn a_base_only_install_latches_only_after_the_totals_hold_still() {
        let mut sampler = CatalogSampler::new();
        for expected_streak in 1..CATALOG_STABLE_SAMPLES_TO_LATCH {
            assert_eq!(
                sampler.observe(Some(EXPECTED_CATALOG_BASE)),
                CatalogSampleStep::Sampled {
                    summary: EXPECTED_CATALOG_BASE,
                    stable_streak: expected_streak,
                },
                "streak {expected_streak} should not have latched yet"
            );
            assert!(!sampler.is_finished());
        }
        assert_eq!(
            sampler.observe(Some(EXPECTED_CATALOG_BASE)),
            CatalogSampleStep::Latched {
                summary: EXPECTED_CATALOG_BASE,
                verdict: CatalogFingerprintVerdict::MatchBase,
            }
        );
    }

    #[test]
    fn a_dlc_container_that_mounts_late_is_not_latched_as_a_base_only_match() {
        // THE failure this sampler exists to prevent: base parses first, the DLC arrives a few
        // samples later, and a single-read oracle would have already reported 257/4482 MATCH.
        let mut sampler = CatalogSampler::new();
        for _ in 0..CATALOG_STABLE_SAMPLES_TO_LATCH - 1 {
            let step = sampler.observe(Some(EXPECTED_CATALOG_BASE));
            assert!(
                matches!(step, CatalogSampleStep::Sampled { .. }),
                "{step:?}"
            );
        }
        assert_eq!(
            sampler.observe(Some(EXPECTED_CATALOG_BASE_DLC02)),
            CatalogSampleStep::Latched {
                summary: EXPECTED_CATALOG_BASE_DLC02,
                verdict: CatalogFingerprintVerdict::MatchBaseAndDlc02,
            }
        );
    }

    #[test]
    fn a_growing_catalog_resets_the_stability_streak_each_time_it_changes() {
        let mut sampler = CatalogSampler::new();
        assert_eq!(
            sampler.observe(Some(summary(10, 100, 1))),
            CatalogSampleStep::Sampled {
                summary: summary(10, 100, 1),
                stable_streak: 1,
            }
        );
        assert_eq!(
            sampler.observe(Some(summary(10, 100, 1))),
            CatalogSampleStep::Sampled {
                summary: summary(10, 100, 1),
                stable_streak: 2,
            }
        );
        // One more block appears: back to a streak of one.
        assert_eq!(
            sampler.observe(Some(summary(11, 110, 1))),
            CatalogSampleStep::Sampled {
                summary: summary(11, 110, 1),
                stable_streak: 1,
            }
        );
    }

    #[test]
    fn a_stable_but_wrong_total_latches_as_a_mismatch_rather_than_sampling_forever() {
        let wrong = summary(12, 40, 1);
        let mut sampler = CatalogSampler::new();
        for _ in 0..CATALOG_STABLE_SAMPLES_TO_LATCH - 1 {
            sampler.observe(Some(wrong));
        }
        assert_eq!(
            sampler.observe(Some(wrong)),
            CatalogSampleStep::Latched {
                summary: wrong,
                verdict: CatalogFingerprintVerdict::Mismatch,
            }
        );
    }

    #[test]
    fn not_ready_reads_retry_but_only_the_first_few_are_logged() {
        let mut sampler = CatalogSampler::new();
        for attempt in 1..=CATALOG_RETRY_LOG_LIMIT {
            assert_eq!(
                sampler.observe(None),
                CatalogSampleStep::Retry { attempt, log: true }
            );
        }
        assert_eq!(
            sampler.observe(None),
            CatalogSampleStep::Retry {
                attempt: CATALOG_RETRY_LOG_LIMIT + 1,
                log: false,
            }
        );
        assert!(
            !sampler.is_finished(),
            "a not-ready read must keep retrying"
        );
        assert_eq!(sampler.last_summary(), None);
    }

    #[test]
    fn a_never_ready_singleton_gives_up_instead_of_reading_forever() {
        let mut sampler = CatalogSampler::new();
        for _ in 0..CATALOG_SAMPLE_ATTEMPT_LIMIT - 1 {
            assert!(!sampler.is_finished());
            sampler.observe(None);
        }
        assert_eq!(
            sampler.observe(None),
            CatalogSampleStep::GaveUp {
                attempts: CATALOG_SAMPLE_ATTEMPT_LIMIT,
            }
        );
        assert!(sampler.is_finished());
    }

    #[test]
    fn a_catalog_that_never_stops_changing_also_terminates() {
        // Totals that move on every sample can never build a streak; the attempt limit is what
        // stops the walk running on the game thread for the whole session.
        let mut sampler = CatalogSampler::new();
        let mut last = CatalogSampleStep::Finished;
        for blocks in (1usize..).take(CATALOG_SAMPLE_ATTEMPT_LIMIT as usize) {
            last = sampler.observe(Some(summary(blocks, blocks * 10, 1)));
        }
        assert_eq!(
            last,
            CatalogSampleStep::GaveUp {
                attempts: CATALOG_SAMPLE_ATTEMPT_LIMIT,
            }
        );
        assert!(sampler.is_finished());
    }
}
