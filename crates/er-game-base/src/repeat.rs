//! Run-length suppression for log lines that repeat verbatim.
//!
//! # The shape of the problem this solves
//!
//! Some diagnostics sit on a polled path: the game asks the same question every frame and
//! the hook answers it identically every frame. Measured on run `br-20260831-160354-2513`
//! (8m39s), `combined_load_67b940` was entered 8,639 times with `slot=-1 arg1=0 arg2=1`
//! every single time, from one caller stack, and the 8,639 `ENTER` lines had exactly
//! **seven** distinct contents between them. The trace cost 5.43 MB (37 MB/h) to carry
//! seven facts.
//!
//! # What is memoised, and what is not
//!
//! The suppression predicate is **full-message byte equality against the previous message
//! under the same key** -- the value, not a label. This is the same governing rule as the
//! address-translation memo (bd
//! `memoise-on-the-value-not-the-label-one-label-covered-9-addresses-2026-08-30`), where
//! keying on the human-readable label would have suppressed 8 of 9 genuinely different
//! addresses that shared one label. Here the key selects only which slot remembers the last
//! message; it is never itself the thing compared.
//!
//! That distinction is what makes a key collision harmless. Two families that hash to one
//! slot interleave, their messages differ, nothing matches, and both are emitted -- the
//! filter fails open into today's behaviour. The only case where a collision suppresses
//! anything is two call sites emitting byte-identical text back to back, and byte-identical
//! text carries no distinct fact. There is no input for which this filter drops information
//! that the surviving lines plus their counts do not carry.
//!
//! # Counts are mandatory, not decoration
//!
//! A reader must never mistake "3 lines" for "3 calls". Every suppressed run is accounted
//! for twice over: a restatement on a 1-3-10 schedule while the run is still going (so
//! magnitude reaches the log even if the process dies mid-run, under-reporting by at most
//! 3x), and an exact total the moment the run ends.
//!
//! # Cost
//!
//! Formatting the message and taking one mutex per line. That is deliberately not the shape
//! chosen for the address path, which was 145,006 events and got a lock-free bitset; this
//! one replaces an `open()`+`write()`+`close()` per line, so a mutex is orders of magnitude
//! cheaper than what it removes. It is not for per-frame paths that currently do no I/O.

use std::collections::HashMap;
use std::sync::Mutex;

/// Distinct keys one filter will track. Past this the filter stops learning new families and
/// emits them unconditionally -- degraded to today's behaviour rather than evicting a live
/// run whose total would then never be reported. Measured key counts on real logs: 95
/// (continue trace), 275 (autoload debug).
pub const KEY_CAPACITY: usize = 512;

/// What the caller should write for one observed message.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Write `note` first when present (it closes out a finished run), then the message.
    Emit { note: Option<String> },
    /// Write only `note`: the message is a verbatim repeat, and this states the magnitude.
    Note(String),
    /// Write nothing.
    Suppress,
}

struct Slot {
    /// Printable family name, kept so a report line can say which family repeated.
    key: String,
    message: String,
    /// Occurrences of `message` in a row, including the one that was emitted.
    run: u64,
    /// Highest run length already restated, so each milestone fires once.
    restated: u64,
}

/// One run-length filter. Hold it in a `static` beside the writer it guards.
pub struct RepeatFilter {
    slots: Mutex<Option<HashMap<u64, Slot>>>,
    capacity: usize,
}

impl RepeatFilter {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Mutex::new(None),
            capacity: KEY_CAPACITY,
        }
    }

    #[must_use]
    pub const fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Mutex::new(None),
            capacity,
        }
    }

    /// Decide what to write for `message`.
    pub fn observe(&self, message: &str) -> Verdict {
        let key_hash = family_key_hash(message);
        let mut guard = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slots = guard.get_or_insert_with(HashMap::new);

        if let Some(slot) = slots.get_mut(&key_hash) {
            if slot.message == message {
                slot.run += 1;
                return if next_milestone(slot.restated) == slot.run {
                    slot.restated = slot.run;
                    Verdict::Note(format!(
                        "repeat: {} -- still emitting an identical line, {} occurrences so far",
                        slot.key, slot.run
                    ))
                } else {
                    Verdict::Suppress
                };
            }
            let note = (slot.run > 1).then(|| {
                format!(
                    "repeat: {} -- the identical line above occurred {} times",
                    slot.key, slot.run
                )
            });
            slot.message.clear();
            slot.message.push_str(message);
            slot.run = 1;
            slot.restated = 1;
            // The family is unchanged on this branch only when the key matched, which is the
            // case here; a colliding family simply re-labels its own slot below.
            slot.key.clear();
            slot.key.push_str(&family_key(message));
            return Verdict::Emit { note };
        }

        if slots.len() >= self.capacity {
            // Fail open. Evicting would strand a live run whose exact total is only ever
            // reported when that run ends.
            return Verdict::Emit { note: None };
        }
        slots.insert(
            key_hash,
            Slot {
                key: family_key(message),
                message: message.to_owned(),
                run: 1,
                restated: 1,
            },
        );
        Verdict::Emit { note: None }
    }
}

impl Default for RepeatFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Next run length worth restating, on a 1-3-10 schedule (10, 30, 100, 300, 1000, ...).
///
/// A plain power-of-ten schedule under-reports a run cut short by process exit by up to 10x;
/// interleaving the 3s halves that to 3x for one extra line per decade.
fn next_milestone(restated: u64) -> u64 {
    let mut milestone = 10u64;
    loop {
        if milestone > restated {
            return milestone;
        }
        milestone = match milestone.checked_mul(3) {
            Some(three) if three > restated => return three,
            Some(three) => three,
            None => return u64::MAX,
        };
        milestone = match milestone.checked_mul(10).map(|ten| ten / 3) {
            Some(next) => next,
            None => return u64::MAX,
        };
    }
}

/// Printable family name: the first two whitespace-separated tokens, with every run
/// containing a digit collapsed to `#`.
///
/// The collapse is what keeps the key space bounded. `menu_task_enqueue seq=1`, `seq=2`, ...
/// would otherwise mint a fresh family per line and fill the table with singletons; as
/// `menu_task_enqueue seq=#` they share one slot and simply never match each other, which is
/// the fail-open case.
#[must_use]
pub fn family_key(message: &str) -> String {
    let mut key = String::new();
    for (index, token) in message.split_whitespace().take(2).enumerate() {
        if index > 0 {
            key.push(' ');
        }
        push_normalised(&mut key, token);
    }
    key
}

fn push_normalised(out: &mut String, token: &str) {
    let mut run_start: Option<usize> = None;
    let mut run_has_digit = false;
    for (index, byte) in token.bytes().enumerate() {
        let hexish = byte.is_ascii_hexdigit();
        if hexish {
            if run_start.is_none() {
                run_start = Some(index);
                run_has_digit = false;
            }
            run_has_digit |= byte.is_ascii_digit();
        } else {
            flush_run(out, token, run_start.take(), run_has_digit, index);
            out.push(byte as char);
        }
    }
    flush_run(out, token, run_start.take(), run_has_digit, token.len());
}

fn flush_run(out: &mut String, token: &str, start: Option<usize>, has_digit: bool, end: usize) {
    let Some(start) = start else {
        return;
    };
    if has_digit {
        out.push('#');
    } else {
        out.push_str(&token[start..end]);
    }
}

/// FNV-1a over [`family_key`] without building it. Allocation-free, so the suppressed path
/// (the common one, by construction) allocates nothing at all.
fn family_key_hash(message: &str) -> u64 {
    let mut hash = crate::fnv1a::FNV1A64_OFFSET_BASIS;
    let mut absorb = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(crate::fnv1a::FNV1A64_PRIME);
    };
    for (index, token) in message.split_whitespace().take(2).enumerate() {
        if index > 0 {
            absorb(b' ');
        }
        let bytes = token.as_bytes();
        let mut run_start: Option<usize> = None;
        let mut run_has_digit = false;
        for (position, byte) in bytes.iter().copied().enumerate() {
            if byte.is_ascii_hexdigit() {
                if run_start.is_none() {
                    run_start = Some(position);
                    run_has_digit = false;
                }
                run_has_digit |= byte.is_ascii_digit();
                continue;
            }
            absorb_run(
                &mut absorb,
                bytes,
                run_start.take(),
                run_has_digit,
                position,
            );
            absorb(byte);
        }
        absorb_run(
            &mut absorb,
            bytes,
            run_start.take(),
            run_has_digit,
            bytes.len(),
        );
    }
    hash
}

/// Feed one hex-ish run to the hash exactly as [`push_normalised`] would render it: a single
/// `#` when the run contained a decimal digit, the literal bytes otherwise.
fn absorb_run(
    absorb: &mut impl FnMut(u8),
    bytes: &[u8],
    start: Option<usize>,
    has_digit: bool,
    end: usize,
) {
    let Some(start) = start else {
        return;
    };
    if has_digit {
        absorb(b'#');
        return;
    }
    for byte in bytes[start..end].iter().copied() {
        absorb(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing property: a run of identical lines collapses, and the count of
    /// occurrences survives so a reader never mistakes emitted lines for calls.
    #[test]
    fn an_identical_run_collapses_but_its_total_is_reported() {
        let filter = RepeatFilter::new();
        assert_eq!(
            filter.observe("ENTER combined_load_67b940 slot=-1"),
            Verdict::Emit { note: None }
        );
        let mut written = 1;
        for _ in 0..8_638 {
            match filter.observe("ENTER combined_load_67b940 slot=-1") {
                Verdict::Suppress => {}
                Verdict::Note(note) => {
                    assert!(note.contains("occurrences so far"), "{note}");
                    written += 1;
                }
                other => panic!("a verbatim repeat must not emit the line again: {other:?}"),
            }
        }
        assert!(
            written < 20,
            "8,639 identical lines collapsed to {written}, which is not a bound"
        );
        let Verdict::Emit { note: Some(note) } =
            filter.observe("ENTER combined_load_67b940 slot=2")
        else {
            panic!("a changed line must be emitted, and must close out the run it ended");
        };
        assert!(
            note.contains("8639"),
            "the exact occurrence count must survive: {note}"
        );
    }

    /// A run that never ends -- the process dies mid-poll -- still has to put its magnitude in
    /// the log, or a reader who sees one line concludes one call. Without this, a filter that
    /// simply never restates passes every other test here.
    #[test]
    fn a_run_cut_short_by_process_exit_still_reported_its_magnitude() {
        let filter = RepeatFilter::new();
        let mut last_reported = 0u64;
        for _ in 0..8_639 {
            if let Verdict::Note(note) = filter.observe("ENTER combined_load_67b940 slot=-1") {
                last_reported = note
                    .split_whitespace()
                    .find_map(|word| word.parse::<u64>().ok())
                    .expect("a magnitude restatement must carry the occurrence count");
            }
        }
        assert!(
            last_reported >= 3_000,
            "8,639 occurrences left {last_reported} as the last magnitude in the log; a reader \
             cut off here under-reports by more than the 3x the schedule promises"
        );
    }

    /// Non-VACUITY: the same assertion must go red against a filter that suppresses on the
    /// key rather than on the message. That is the failure mode the address-translation memo
    /// was written about, and this test is the only thing stopping it coming back here.
    #[test]
    fn a_changed_message_under_one_key_is_never_suppressed() {
        let filter = RepeatFilter::new();
        for slot in 0..64 {
            let line = format!("ENTER combined_load_67b940 slot={slot}");
            assert!(
                matches!(filter.observe(&line), Verdict::Emit { .. }),
                "slot={slot} shares a family key with its predecessors and was suppressed; \
                 the filter is keying on the label, not the value"
            );
        }
    }

    /// Interleaved families must each get their own slot, or the polled ENTER/LEAVE pair --
    /// the exact shape this exists for -- never dedupes at all.
    #[test]
    fn interleaved_families_do_not_defeat_each_other() {
        let filter = RepeatFilter::new();
        let mut emitted = 0;
        for _ in 0..1_000 {
            for line in [
                "ENTER combined_load_67b940 a=1",
                "LEAVE combined_load_67b940 ret=0",
            ] {
                if !matches!(filter.observe(line), Verdict::Suppress) {
                    emitted += 1;
                }
            }
        }
        assert!(
            emitted < 30,
            "2,000 interleaved-but-repeating lines produced {emitted} writes"
        );
    }

    /// A family whose tail varies per line must pass straight through. `menu_task_enqueue
    /// seq=N` is the real one: it shares a key by design and must not lose lines to it.
    #[test]
    fn a_family_that_never_repeats_is_never_suppressed() {
        let filter = RepeatFilter::new();
        for seq in 0..500 {
            let line = format!("menu_task_enqueue seq={seq} phase=ENTER");
            assert!(
                matches!(filter.observe(&line), Verdict::Emit { .. }),
                "seq={seq} was suppressed although no line repeated"
            );
        }
    }

    /// Past capacity the filter must degrade to emitting everything, not start evicting live
    /// runs whose totals would then never be reported.
    #[test]
    fn beyond_capacity_it_fails_open() {
        let filter = RepeatFilter::with_capacity(4);
        for family in 0..64 {
            let line = format!("family{family}x aaa");
            assert!(matches!(filter.observe(&line), Verdict::Emit { .. }));
            assert!(
                matches!(
                    filter.observe(&line),
                    Verdict::Emit { .. } | Verdict::Suppress
                ),
                "an over-capacity family must still be written, never dropped"
            );
        }
    }

    #[test]
    fn family_keys_collapse_digit_runs_and_split_on_the_second_token() {
        assert_eq!(
            family_key("ENTER combined_load_67b940 slot=-1"),
            "ENTER combined_load_#"
        );
        assert_eq!(
            family_key("LEAVE combined_load_67b940 ret=0"),
            "LEAVE combined_load_#"
        );
        assert_eq!(
            family_key("menu_task_enqueue seq=12 phase=ENTER"),
            "menu_task_enqueue seq=#"
        );
        assert_ne!(
            family_key("ENTER combined_load_67b940 x"),
            family_key("ENTER continue_load_67b750 x"),
            "two different hooks must not share a family"
        );
    }

    /// The hash must agree with the string it claims to hash, or two families silently share
    /// a slot and the fail-open path is exercised far more often than measured.
    #[test]
    fn the_allocation_free_hash_matches_the_printable_key() {
        for message in [
            "ENTER combined_load_67b940 slot=-1 arg1=0",
            "LEAVE combined_load_67b940 ret=0 gm=0x8406c080",
            "menu_task_enqueue seq=1234 phase=ENTER",
            "SEQ-ITER-DBG #3 seq=0x1",
            "save-override: save-picker mode from ERSC module latch",
            "single",
            "",
        ] {
            let mut expected = crate::fnv1a::FNV1A64_OFFSET_BASIS;
            for byte in family_key(message).bytes() {
                expected ^= u64::from(byte);
                expected = expected.wrapping_mul(crate::fnv1a::FNV1A64_PRIME);
            }
            assert_eq!(
                family_key_hash(message),
                expected,
                "hash disagrees with family_key({message:?}) = {:?}",
                family_key(message)
            );
        }
    }

    #[test]
    fn milestones_follow_a_one_three_ten_schedule() {
        let mut seen = Vec::new();
        let mut at = 1u64;
        for _ in 0..8 {
            at = next_milestone(at);
            seen.push(at);
        }
        assert_eq!(seen, vec![10, 30, 100, 300, 1_000, 3_000, 10_000, 30_000]);
    }
}
