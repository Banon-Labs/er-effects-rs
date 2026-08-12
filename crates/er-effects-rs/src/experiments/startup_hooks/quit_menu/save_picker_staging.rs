use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PickerRowStageSnapshot {
    labels: Vec<Vec<u16>>,
    description: String,
}

impl PickerRowStageSnapshot {
    pub(crate) fn from_model(model: &crate::experiments::save_picker::SavePickerModel) -> Self {
        let visible = model
            .visible_row_count()
            .min(TITLE_PROFILE_SLOT_COUNT)
            .min(crate::experiments::save_picker::PICKER_ROW_COUNT);
        Self {
            labels: (0..visible)
                .map(|slot| model.row_label_utf16(slot))
                .collect(),
            description: format!(
                "dir='{}' scroll={}/{} entries={} drives={}",
                model.current_dir().display(),
                model.scroll_offset(),
                model.scroll_max(),
                model.entry_count(),
                model.drive_count()
            ),
        }
    }

    #[cfg(test)]
    fn from_test_labels(labels: &[&str]) -> Self {
        Self {
            labels: labels
                .iter()
                .map(|label| label.encode_utf16().collect())
                .collect(),
            description: labels.join(","),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PickerPresentationAttempt {
    prior: PickerRowStageSnapshot,
    candidate: PickerRowStageSnapshot,
    applied: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PickerPresentationState {
    current: Option<PickerRowStageSnapshot>,
    inflight: Option<PickerPresentationAttempt>,
    render_override: Option<PickerRowStageSnapshot>,
}

static SAVE_PICKER_PRESENTATION_STATE: std::sync::Mutex<PickerPresentationState> =
    std::sync::Mutex::new(PickerPresentationState {
        current: None,
        inflight: None,
        render_override: None,
    });

fn picker_presentation_state() -> std::sync::MutexGuard<'static, PickerPresentationState> {
    SAVE_PICKER_PRESENTATION_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PickerPresentationTestSnapshot(PickerPresentationState);

#[cfg(test)]
pub(crate) fn snapshot_picker_presentation_for_test() -> PickerPresentationTestSnapshot {
    PickerPresentationTestSnapshot(picker_presentation_state().clone())
}

#[cfg(test)]
pub(crate) fn restore_picker_presentation_for_test(snapshot: &PickerPresentationTestSnapshot) {
    *picker_presentation_state() = snapshot.0.clone();
}

fn picker_render_override() -> Option<PickerRowStageSnapshot> {
    picker_presentation_state().render_override.clone()
}

fn set_picker_render_override(snapshot: Option<PickerRowStageSnapshot>) {
    picker_presentation_state().render_override = snapshot;
}

fn record_picker_presentation_with(
    state: &std::sync::Mutex<PickerPresentationState>,
    snapshot: PickerRowStageSnapshot,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.inflight.is_some() {
        return false;
    }
    state.current = Some(snapshot);
    true
}

fn begin_picker_presentation_attempt_with(
    state: &std::sync::Mutex<PickerPresentationState>,
    candidate: PickerRowStageSnapshot,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(prior) = state.current.clone() else {
        return false;
    };
    if state.inflight.is_some() {
        return false;
    }
    state.inflight = Some(PickerPresentationAttempt {
        prior,
        candidate,
        // Treat the attempt as mutating before its first write. Even a fallible/partial writer must
        // reconstruct the preceding value-owned presentation on failure.
        applied: true,
    });
    true
}

fn commit_picker_presentation_attempt_with(
    state: &std::sync::Mutex<PickerPresentationState>,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(attempt) = state.inflight.take() else {
        return false;
    };
    state.current = Some(attempt.candidate);
    true
}

fn rollback_picker_presentation_attempt_with(
    state: &std::sync::Mutex<PickerPresentationState>,
    mut restore: impl FnMut(&PickerRowStageSnapshot) -> bool,
) -> bool {
    let attempt = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.inflight.take()
    };
    let Some(attempt) = attempt else {
        return false;
    };
    let restored = !attempt.applied || restore(&attempt.prior);
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.current = restored.then_some(attempt.prior);
    restored
}

fn clear_picker_presentation_with(state: &std::sync::Mutex<PickerPresentationState>) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.inflight.is_some() {
        return false;
    }
    *state = PickerPresentationState::default();
    true
}

pub(crate) fn clear_picker_presentation() {
    let _ = clear_picker_presentation_with(&SAVE_PICKER_PRESENTATION_STATE);
}

unsafe fn save_picker_write_rows_with(snapshot: &PickerRowStageSnapshot, summary: usize) -> usize {
    let visible = snapshot
        .labels
        .len()
        .min(TITLE_PROFILE_SLOT_COUNT)
        .min(crate::experiments::save_picker::PICKER_ROW_COUNT);
    unsafe {
        for slot in 0..TITLE_PROFILE_SLOT_COUNT {
            let record =
                summary + PROFILE_SUMMARY_RECORD_BASE + slot * PROFILE_SUMMARY_RECORD_STRIDE;
            core::ptr::write_bytes(record as *mut u8, 0, PROFILE_SUMMARY_RECORD_STRIDE);
            PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
            if slot >= visible {
                *((summary + PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot) as *mut u8) = 0;
                continue;
            }
            let label = &snapshot.labels[slot];
            let fallback: Vec<u16>;
            let label = if label.is_empty() {
                fallback = "-".encode_utf16().collect();
                &fallback
            } else {
                label
            };
            let units = label.len().min(PROFILE_SUMMARY_NAME_BYTES / 2 - 1);
            core::ptr::copy_nonoverlapping(label.as_ptr(), record as *mut u16, units);
            *((summary + PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot) as *mut u8) = 1;
        }
    }
    visible
}

/// Pure ProfileSummary record transport; no renderer call. Records the value-owned picker rows so
/// a later failed resubmit can reconstruct this exact presentation instead of vanilla rows.
pub(crate) unsafe fn save_picker_write_row_records(
    model: &crate::experiments::save_picker::SavePickerModel,
    summary: usize,
) -> usize {
    let override_snapshot = picker_render_override();
    let snapshot = override_snapshot
        .clone()
        .unwrap_or_else(|| PickerRowStageSnapshot::from_model(model));
    let staged = unsafe { save_picker_write_rows_with(&snapshot, summary) };
    if override_snapshot.is_none() {
        let _ = record_picker_presentation_with(&SAVE_PICKER_PRESENTATION_STATE, snapshot);
    }
    staged
}

pub(crate) fn save_picker_stage_owner_is_clear(profile_select_owner: usize) -> bool {
    profile_select_owner == 0
}

unsafe fn save_picker_render_snapshot(snapshot: &PickerRowStageSnapshot) -> bool {
    let profile_select_owner = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if !save_picker_stage_owner_is_clear(profile_select_owner) {
        append_autoload_debug(format_args!(
            "save-picker: row staging REJECTED while old ProfileSelect owner=0x{profile_select_owner:x} remains live"
        ));
        return false;
    }
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-picker: cannot stage rows -- live ProfileSummary unavailable"
        ));
        return false;
    }
    {
        let mut st = system_quit_save_swap_lock();
        if st.summary_snapshot.is_empty() || st.summary_ptr != summary {
            st.summary_ptr = summary;
            st.summary_snapshot = unsafe {
                core::slice::from_raw_parts(summary as *const u8, PROFILE_SUMMARY_TOTAL_BYTES)
                    .to_vec()
            };
        }
    }
    let staged = unsafe { save_picker_write_rows_with(snapshot, summary) };
    {
        let mut st = system_quit_save_swap_lock();
        if st.arm_generation == 0 {
            st.next_generation = st.next_generation.wrapping_add(1).max(1);
            st.arm_generation = st.next_generation;
        }
        st.summary_mutated_generation = st.arm_generation;
    }
    SAVE_PICKER_STAGED_ROW_COUNT.store(staged, Ordering::SeqCst);
    SAVE_PICKER_LAYOUT_GENERATION.fetch_add(1, Ordering::SeqCst);
    set_picker_render_override(Some(snapshot.clone()));
    if let Ok(base) = game_module_base() {
        let refresh: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
        unsafe { refresh() };
    }
    set_picker_render_override(None);
    append_autoload_debug(format_args!(
        "save-picker: staged {staged} rows ({} unoccupied) {}",
        TITLE_PROFILE_SLOT_COUNT.saturating_sub(staged),
        snapshot.description
    ));
    true
}

pub(crate) unsafe fn save_picker_stage_row_records(
    model: &crate::experiments::save_picker::SavePickerModel,
) -> bool {
    let snapshot = PickerRowStageSnapshot::from_model(model);
    if !unsafe { save_picker_render_snapshot(&snapshot) } {
        return false;
    }
    let _ = record_picker_presentation_with(&SAVE_PICKER_PRESENTATION_STATE, snapshot);
    true
}

/// Begin the resubmit's live-row attempt. The exact preceding picker presentation is retained as
/// owned labels, and no native pointer or compiler-private structure crosses the attempt boundary.
pub(crate) unsafe fn save_picker_stage_row_snapshot(snapshot: &PickerRowStageSnapshot) -> bool {
    if !begin_picker_presentation_attempt_with(&SAVE_PICKER_PRESENTATION_STATE, snapshot.clone()) {
        return false;
    }
    unsafe { save_picker_render_snapshot(snapshot) }
}

pub(crate) fn commit_picker_staged_presentation() -> bool {
    commit_picker_presentation_attempt_with(&SAVE_PICKER_PRESENTATION_STATE)
}

pub(crate) unsafe fn rollback_picker_staged_presentation() -> bool {
    rollback_picker_presentation_attempt_with(&SAVE_PICKER_PRESENTATION_STATE, |prior| unsafe {
        save_picker_render_snapshot(prior)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_reset_clears_only_after_false_rollback_or_true_commit() {
        for submit_ok in [false, true] {
            let presentation = std::sync::Mutex::new(PickerPresentationState::default());
            let a = PickerRowStageSnapshot::from_test_labels(&["A"]);
            let b = PickerRowStageSnapshot::from_test_labels(&["B"]);
            assert!(record_picker_presentation_with(&presentation, a.clone()));
            assert!(begin_picker_presentation_attempt_with(
                &presentation,
                b.clone()
            ));
            // Synchronous reset while reserved records intent elsewhere; even a direct defensive
            // clear cannot mutate current/inflight or create a clear-after-commit side effect.
            assert!(!clear_picker_presentation_with(&presentation));
            {
                let state = presentation.lock().unwrap();
                assert_eq!(state.current, Some(a.clone()));
                assert_eq!(
                    state.inflight.as_ref().map(|attempt| &attempt.candidate),
                    Some(&b)
                );
            }
            if submit_ok {
                assert!(commit_picker_presentation_attempt_with(&presentation));
                assert_eq!(presentation.lock().unwrap().current, Some(b));
            } else {
                assert!(rollback_picker_presentation_attempt_with(
                    &presentation,
                    |_| true
                ));
                assert_eq!(presentation.lock().unwrap().current, Some(a));
            }
            assert!(clear_picker_presentation_with(&presentation));
            let state = presentation.lock().unwrap();
            assert!(state.current.is_none());
            assert!(state.inflight.is_none());
        }
    }

    #[test]
    fn failed_lost_and_false_submit_restore_exact_prior_then_retry_c_commits_once() {
        let presentation = std::sync::Mutex::new(PickerPresentationState::default());
        let a = PickerRowStageSnapshot::from_test_labels(&["A0", "A1"]);
        let b = PickerRowStageSnapshot::from_test_labels(&["B0", "B1", "B2"]);
        let c = PickerRowStageSnapshot::from_test_labels(&["C0"]);
        assert!(record_picker_presentation_with(&presentation, a.clone()));
        let rendered = std::cell::RefCell::new(a.labels.clone());
        let refreshes = AtomicUsize::new(0);
        let submit_attempts = AtomicUsize::new(0);
        let submit_successes = AtomicUsize::new(0);
        let coordinator = PickerOwnerLifetimeCoordinator::default();

        let render = |snapshot: &PickerRowStageSnapshot| {
            *rendered.borrow_mut() = snapshot.labels.clone();
            refreshes.fetch_add(1, Ordering::SeqCst);
            true
        };

        // Stage failure and authority loss still reconstruct exact A without a native call.
        for (stage_ok, submit_ok, expected) in [
            (false, true, PickerResubmitDisposition::StageFailed),
            (true, false, PickerResubmitDisposition::AuthorizationLost),
        ] {
            assert_eq!(
                execute_owner_zero_resubmit_transaction_on_coordinator_with(
                    &coordinator,
                    || true,
                    || Some(()),
                    || {
                        assert!(begin_picker_presentation_attempt_with(
                            &presentation,
                            b.clone()
                        ));
                        assert!(render(&b));
                        stage_ok
                    },
                    || submit_ok,
                    || {},
                    |_| {},
                    |_| {},
                    || {
                        assert!(rollback_picker_presentation_attempt_with(
                            &presentation,
                            &render
                        ));
                    },
                    || panic!("failure before submit cannot commit presentation"),
                    || panic!("failure before submit cannot call native submit"),
                    |_| PickerOwnerApplyResult::Stale { actual: 0 },
                ),
                expected
            );
            assert_eq!(*rendered.borrow(), a.labels);
            assert_eq!(presentation.lock().unwrap().current, Some(a.clone()));
        }

        let path_generation = AtomicUsize::new(7);
        let refresh_generation = AtomicUsize::new(8);
        let refresh_close_generation = AtomicUsize::new(8);
        let reopen_pending = AtomicUsize::new(1);
        let open_slots_pending = AtomicUsize::new(0);
        let pending_transition = AtomicUsize::new(10);
        let reserved = std::cell::Cell::new(false);
        let model_retained = std::cell::Cell::new(true);
        let exact_latches_present = || {
            path_generation.load(Ordering::SeqCst) == 7
                && refresh_generation.load(Ordering::SeqCst) == 8
                && refresh_close_generation.load(Ordering::SeqCst) == 8
                && reopen_pending.load(Ordering::SeqCst) == 1
                && open_slots_pending.load(Ordering::SeqCst) == 0
                && pending_transition.load(Ordering::SeqCst) == 10
        };

        // Native false owns the exclusive retry reservation through B and exact A rollback. A
        // competing writer is nonblocking and cannot consume the transition.
        assert_eq!(
            execute_owner_zero_resubmit_transaction_on_coordinator_with(
                &coordinator,
                || true,
                || {
                    if !exact_latches_present() || reserved.replace(true) {
                        None
                    } else {
                        Some(())
                    }
                },
                || {
                    assert!(begin_picker_presentation_attempt_with(
                        &presentation,
                        b.clone()
                    ));
                    assert!(render(&b));
                    if !reserved.get() {
                        reopen_pending.store(0, Ordering::SeqCst);
                    }
                    true
                },
                || true,
                || {},
                |_| {
                    assert!(reserved.replace(false));
                    path_generation.store(0, Ordering::SeqCst);
                    refresh_generation.store(0, Ordering::SeqCst);
                    refresh_close_generation.store(0, Ordering::SeqCst);
                    reopen_pending.store(0, Ordering::SeqCst);
                    open_slots_pending.store(0, Ordering::SeqCst);
                    pending_transition.store(0, Ordering::SeqCst);
                },
                |_| reserved.set(false),
                || {
                    assert!(rollback_picker_presentation_attempt_with(
                        &presentation,
                        &render
                    ));
                },
                || panic!("false submit cannot commit B"),
                || {
                    submit_attempts.fetch_add(1, Ordering::SeqCst);
                    false
                },
                |_| PickerOwnerApplyResult::Stale { actual: 0 },
            ),
            PickerResubmitDisposition::Submitted { opened: false }
        );
        crate::save_picker_menu::apply_picker_resubmit_model_lifetime_with(
            PickerResubmitDisposition::Submitted { opened: false },
            true,
            || model_retained.set(false),
        );
        assert_eq!(*rendered.borrow(), a.labels);
        assert_eq!(presentation.lock().unwrap().current, Some(a.clone()));
        assert!(exact_latches_present());
        assert!(!reserved.get());
        assert!(model_retained.get());
        assert_eq!(submit_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(submit_successes.load(Ordering::SeqCst), 0);

        // The exact retry stages C and a true submit makes latch + presentation commit one
        // consequence of the still-owned reservation. Caller cleanup follows success only.
        assert_eq!(
            execute_owner_zero_resubmit_transaction_on_coordinator_with(
                &coordinator,
                || true,
                || {
                    if !exact_latches_present() || reserved.replace(true) {
                        None
                    } else {
                        Some(())
                    }
                },
                || {
                    assert!(begin_picker_presentation_attempt_with(
                        &presentation,
                        c.clone()
                    ));
                    render(&c)
                },
                || true,
                || {},
                |_| {
                    assert!(reserved.replace(false));
                    path_generation.store(0, Ordering::SeqCst);
                    refresh_generation.store(0, Ordering::SeqCst);
                    refresh_close_generation.store(0, Ordering::SeqCst);
                    reopen_pending.store(0, Ordering::SeqCst);
                    open_slots_pending.store(0, Ordering::SeqCst);
                    pending_transition.store(0, Ordering::SeqCst);
                },
                |_| reserved.set(false),
                || panic!("exact retry must not roll back"),
                || {
                    assert!(commit_picker_presentation_attempt_with(&presentation));
                },
                || {
                    submit_attempts.fetch_add(1, Ordering::SeqCst);
                    submit_successes.fetch_add(1, Ordering::SeqCst);
                    true
                },
                |_| PickerOwnerApplyResult::Stale { actual: 0 },
            ),
            PickerResubmitDisposition::Submitted { opened: true }
        );
        crate::save_picker_menu::apply_picker_resubmit_model_lifetime_with(
            PickerResubmitDisposition::Submitted { opened: true },
            true,
            || model_retained.set(false),
        );
        assert_eq!(*rendered.borrow(), c.labels);
        assert_eq!(presentation.lock().unwrap().current, Some(c.clone()));
        assert_eq!(path_generation.load(Ordering::SeqCst), 0);
        assert_eq!(refresh_generation.load(Ordering::SeqCst), 0);
        assert_eq!(refresh_close_generation.load(Ordering::SeqCst), 0);
        assert_eq!(reopen_pending.load(Ordering::SeqCst), 0);
        assert_eq!(open_slots_pending.load(Ordering::SeqCst), 0);
        assert_eq!(pending_transition.load(Ordering::SeqCst), 0);
        assert!(!reserved.get());
        assert!(!model_retained.get());
        assert_eq!(submit_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(submit_successes.load(Ordering::SeqCst), 1);
        assert_eq!(refreshes.load(Ordering::SeqCst), 7);
    }
}
