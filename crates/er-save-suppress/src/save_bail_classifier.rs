// Which OPERAND refused the save, from one sample of the SL device's request slot.
//
// `include!`d into `lib.rs` like the blocks around it, and split out of it so the classifier and
// its labels sit together rather than in the middle of the observers that consume them.
//
// The submit builders (`FUN_140e6ef60`, `FUN_140e6ec70`) open on
// `iodev+0x10 == 0 && iodev+0x20 == 0` plus four operands the call site guarantees statically, so
// when a lane returns 0 those two fields are the only ones that can have failed -- Provided the
// lane reached the builder at all. `dispatch_refusal_is_the_mutex` in `save_state_device.rs` is
// the clause that decides whether it did; read that first, then this.

/// No decline has been classified yet.
pub const SL_BAIL_UNSAMPLED: usize = 0;
/// The device pointer or its fields could not be read; the sample proves nothing.
pub const SL_BAIL_IODEV_UNREADABLE: usize = 1;
/// `iodev+0x10` still holds an `SLSaveContent`: a previous save request was never released.
pub const SL_BAIL_SAVE_CONTENT_LATCHED: usize = 2;
/// `iodev+0x20` holds a job and `iodev+0x18` holds load content: a completed load was never
/// consumed, and it is occupying the shared job slot the save needs.
pub const SL_BAIL_LOAD_JOB_LATCHED: usize = 3;
/// `iodev+0x20` holds a job with no content on either side -- an orphaned job.
pub const SL_BAIL_JOB_LATCHED: usize = 4;
/// Both guard operands are populated.
pub const SL_BAIL_SAVE_CONTENT_AND_JOB_LATCHED: usize = 5;
/// The precondition holds, so the builder got past its guard: the refusal is the
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

/// Say which operand of the submit builders' precondition failed, from one slot sample.
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
