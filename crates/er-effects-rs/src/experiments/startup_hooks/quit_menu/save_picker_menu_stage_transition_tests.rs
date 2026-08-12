use super::save_flow_menu_stage_cas;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

include!("save_picker_native_removal_tests.rs");

fn apply_test_owner_publication(
    published: &AtomicUsize,
    applied: &AtomicUsize,
    request: PickerOwnerPublicationRequest,
) -> PickerOwnerApplyResult {
    let result = match request {
        PickerOwnerPublicationRequest::Set { new_dialog, .. } => {
            let previous = published.swap(new_dialog, Ordering::SeqCst);
            PickerOwnerApplyResult::Published(PickerOwnerAppliedPublication {
                previous,
                cancelled_close: None,
                lifecycle_generation: 1,
            })
        }
        PickerOwnerPublicationRequest::CompareSet {
            expected,
            new_dialog,
        }
        | PickerOwnerPublicationRequest::CompareRemove {
            expected:
                PickerNativeRemovalCapture {
                    owner:
                        PickerOwnerLineage {
                            dialog: expected, ..
                        },
                    ..
                },
            new_dialog,
            ..
        } => match published.compare_exchange(
            expected,
            new_dialog,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(previous) => PickerOwnerApplyResult::Published(PickerOwnerAppliedPublication {
                previous,
                cancelled_close: None,
                lifecycle_generation: 1,
            }),
            Err(actual) => PickerOwnerApplyResult::Stale { actual },
        },
    };
    if matches!(result, PickerOwnerApplyResult::Published(_)) {
        applied.fetch_add(1, Ordering::SeqCst);
    }
    result
}

fn apply_test_system_dialog(published: &AtomicUsize, applied: &AtomicUsize, dialog: usize) {
    published.store(dialog, Ordering::SeqCst);
    applied.fetch_add(1, Ordering::SeqCst);
}

fn try_test_reservation(
    reset: &PickerResetTransactionCoordinator,
    serialization: &std::sync::Mutex<()>,
    reserved: &std::cell::Cell<bool>,
) -> bool {
    let _serialization = serialization.lock().unwrap();
    if !reset.reservation_allowed() || reserved.get() {
        return false;
    }
    reserved.set(true);
    true
}

#[test]
fn deferred_restore_action_outranks_state_only_and_coalesces_exactly_once() {
    let pending = std::sync::Mutex::new(None);
    record_deferred_reset_action_with(
        &pending,
        PickerDeferredResetAction::PickerState {
            source: "state-first".to_owned(),
        },
    );
    record_deferred_reset_action_with(
        &pending,
        PickerDeferredResetAction::RestoreRealWindows {
            base: 0x140000000,
            source: "restore".to_owned(),
        },
    );
    record_deferred_reset_action_with(
        &pending,
        PickerDeferredResetAction::PickerState {
            source: "state-late".to_owned(),
        },
    );
    assert_eq!(
        pending.lock().unwrap().take(),
        Some(PickerDeferredResetAction::RestoreRealWindows {
            base: 0x140000000,
            source: "restore".to_owned(),
        })
    );
    assert!(pending.lock().unwrap().is_none());
}

#[test]
fn reset_claim_linearizes_before_reservation_and_guard_drop_unblocks_it() {
    let reset = PickerResetTransactionCoordinator::default();
    let serialization = std::sync::Mutex::new(());
    let reserved = std::cell::Cell::new(false);
    let guard = match reset.begin_with(&serialization, || reserved.get()) {
        PickerResetBegin::Claimed(guard) => guard,
        disposition => panic!("unexpected reset disposition: {disposition:?}"),
    };
    assert!(reset.snapshot().0.is_some());
    assert!(!try_test_reservation(&reset, &serialization, &reserved));
    drop(guard);
    assert_eq!(reset.snapshot(), (None, false));
    assert!(try_test_reservation(&reset, &serialization, &reserved));
}

#[test]
fn owner_reservation_defers_stage_and_native_resets_then_orders_true_and_false_cleanup() {
    for submit_ok in [false, true] {
        let owner = PickerOwnerLifetimeCoordinator::default();
        let reset = PickerResetTransactionCoordinator::default();
        let serialization = std::sync::Mutex::new(());
        let reserved = std::cell::Cell::new(false);
        let model = std::cell::Cell::new(true);
        let latches = std::cell::Cell::new(true);
        let presentation = std::cell::Cell::new("A");
        let events = std::cell::RefCell::new(Vec::new());
        let disposition = execute_owner_zero_resubmit_transaction_on_coordinator_with(
            &owner,
            || true,
            || try_test_reservation(&reset, &serialization, &reserved).then_some(()),
            || {
                presentation.set("B");
                events.borrow_mut().push("stage-B");
                assert!(matches!(
                    reset.begin_with(&serialization, || reserved.get()),
                    PickerResetBegin::Deferred {
                        newly_recorded: true
                    }
                ));
                assert!(model.get());
                assert!(latches.get());
                assert_eq!(presentation.get(), "B");
                true
            },
            || true,
            || {},
            |_| {
                assert!(reserved.replace(false));
                latches.set(false);
            },
            |_| reserved.set(false),
            || {
                presentation.set("A");
                events.borrow_mut().push("rollback-A");
            },
            || {
                events.borrow_mut().push("commit-B");
            },
            || {
                assert!(matches!(
                    reset.begin_with(&serialization, || reserved.get()),
                    PickerResetBegin::Deferred {
                        newly_recorded: false
                    }
                ));
                assert!(model.get());
                assert!(latches.get());
                assert_eq!(presentation.get(), "B");
                submit_ok
            },
            |_| PickerOwnerApplyResult::Stale { actual: 0 },
        );
        assert_eq!(
            disposition,
            PickerResubmitDisposition::Submitted { opened: submit_ok }
        );
        assert!(!reserved.get());
        assert_eq!(reset.snapshot(), (None, true));
        if submit_ok {
            assert_eq!(presentation.get(), "B");
            assert!(!latches.get());
        } else {
            assert_eq!(presentation.get(), "A");
            assert!(latches.get());
        }
        assert!(model.get());

        let deferred = reset
            .claim_deferred_with(&serialization, || reserved.get())
            .expect("one exact deferred reset");
        assert!(!try_test_reservation(&reset, &serialization, &reserved));
        assert!(matches!(
            reset.begin_with(&serialization, || reserved.get()),
            PickerResetBegin::Coalesced
        ));
        model.set(false);
        latches.set(false);
        presentation.set("none");
        events.borrow_mut().push("reset");
        drop(deferred);
        assert!(
            reset
                .claim_deferred_with(&serialization, || reserved.get())
                .is_none()
        );
        assert_eq!(reset.snapshot(), (None, false));
        assert_eq!(presentation.get(), "none");
        assert!(!model.get());
        assert!(!latches.get());
        assert_eq!(
            events.into_inner(),
            if submit_ok {
                vec!["stage-B", "commit-B", "reset"]
            } else {
                vec!["stage-B", "rollback-A", "reset"]
            }
        );
    }
}

#[test]
fn destination_reservation_defers_native_reset_until_false_release_or_true_commit() {
    for submit_ok in [false, true] {
        let system = PickerSystemDialogCoordinator::default();
        let published = AtomicUsize::new(0);
        let applied = AtomicUsize::new(0);
        let identity = match system.publish_with(0x5000, |dialog| {
            apply_test_system_dialog(&published, &applied, dialog)
        }) {
            PickerSystemDialogPublicationDisposition::Published(identity) => identity,
            disposition => panic!("unexpected seed disposition: {disposition:?}"),
        };
        let reset = PickerResetTransactionCoordinator::default();
        let serialization = std::sync::Mutex::new(());
        let destination = std::sync::Mutex::new(PickerDestinationResubmitState::default());
        let reopen = AtomicUsize::new(0);
        let open_slots = AtomicUsize::new(1);
        let model = std::cell::Cell::new(true);
        let events = std::cell::RefCell::new(Vec::new());
        let disposition = execute_picker_destination_resubmit_on_coordinator_with(
            &system,
            0,
            identity,
            || {
                let _serialization = serialization.lock().unwrap();
                if !reset.reservation_allowed() {
                    return None;
                }
                reserve_picker_destination_resubmit_transition_with(
                    &destination,
                    &reopen,
                    &open_slots,
                    identity,
                )
            },
            |reservation| {
                assert!(release_picker_destination_resubmit_reservation_with(
                    &destination,
                    reservation,
                ));
            },
            |reservation| {
                commit_picker_destination_resubmit_reservation_with(
                    &destination,
                    &reopen,
                    &open_slots,
                    reservation,
                );
                events.borrow_mut().push("commit-destination");
            },
            || true,
            || {
                assert!(matches!(
                    reset.begin_with(&serialization, || {
                        destination.lock().unwrap().reservation.is_some()
                    }),
                    PickerResetBegin::Deferred {
                        newly_recorded: true
                    }
                ));
                assert!(model.get());
                assert_eq!(open_slots.load(Ordering::SeqCst), 1);
            },
            || {
                assert!(matches!(
                    reset.begin_with(&serialization, || {
                        destination.lock().unwrap().reservation.is_some()
                    }),
                    PickerResetBegin::Deferred {
                        newly_recorded: false
                    }
                ));
                submit_ok
            },
            |dialog| apply_test_system_dialog(&published, &applied, dialog),
        );
        assert_eq!(
            disposition,
            PickerResubmitDisposition::Submitted { opened: submit_ok }
        );
        assert_eq!(open_slots.load(Ordering::SeqCst), usize::from(!submit_ok));
        assert_eq!(reset.snapshot(), (None, true));
        let deferred = reset
            .claim_deferred_with(&serialization, || {
                destination.lock().unwrap().reservation.is_some()
            })
            .expect("one destination deferred reset");
        open_slots.store(0, Ordering::SeqCst);
        model.set(false);
        events.borrow_mut().push("reset");
        drop(deferred);
        assert!(
            reset
                .claim_deferred_with(&serialization, || false)
                .is_none()
        );
        assert_eq!(reset.snapshot(), (None, false));
        assert_eq!(open_slots.load(Ordering::SeqCst), 0);
        assert!(!model.get());
        assert_eq!(
            events.into_inner(),
            if submit_ok {
                vec!["commit-destination", "reset"]
            } else {
                vec!["reset"]
            }
        );
    }
}

#[test]
fn production_restore_wrapper_defers_without_mutation_then_orders_true_false_and_reset_wins() {
    for submit_ok in [false, true] {
        let owner = PickerOwnerLifetimeCoordinator::default();
        let reset = PickerResetTransactionCoordinator::default();
        let serialization = std::sync::Mutex::new(());
        let reserved = std::cell::Cell::new(false);
        let presentation = std::cell::Cell::new("A");
        let restore_rows = AtomicUsize::new(0);
        let renderer_refreshes = AtomicUsize::new(0);
        let reset_mutations = AtomicUsize::new(0);
        let events = std::cell::RefCell::new(Vec::new());

        let disposition = execute_owner_zero_resubmit_transaction_on_coordinator_with(
            &owner,
            || true,
            || try_test_reservation(&reset, &serialization, &reserved).then_some(()),
            || {
                presentation.set("B");
                events.borrow_mut().push("stage-B");
                assert_eq!(
                    execute_system_quit_restore_reset_with(
                        || reset.begin_with(&serialization, || reserved.get()),
                        |_| {
                            restore_rows.fetch_add(1, Ordering::SeqCst);
                            renderer_refreshes.fetch_add(1, Ordering::SeqCst);
                            reset_mutations.fetch_add(1, Ordering::SeqCst);
                        },
                    ),
                    SystemQuitRestoreResetDisposition::Deferred {
                        newly_recorded: true
                    }
                );
                assert_eq!(presentation.get(), "B");
                assert_eq!(restore_rows.load(Ordering::SeqCst), 0);
                assert_eq!(renderer_refreshes.load(Ordering::SeqCst), 0);
                assert_eq!(reset_mutations.load(Ordering::SeqCst), 0);
                true
            },
            || true,
            || {},
            |_| {
                assert!(reserved.replace(false));
            },
            |_| reserved.set(false),
            || {
                presentation.set("A");
                events.borrow_mut().push("rollback-A");
            },
            || events.borrow_mut().push("commit-B"),
            || {
                assert_eq!(
                    execute_system_quit_restore_reset_with(
                        || reset.begin_with(&serialization, || reserved.get()),
                        |_| {
                            restore_rows.fetch_add(1, Ordering::SeqCst);
                            renderer_refreshes.fetch_add(1, Ordering::SeqCst);
                            reset_mutations.fetch_add(1, Ordering::SeqCst);
                        },
                    ),
                    SystemQuitRestoreResetDisposition::Deferred {
                        newly_recorded: false
                    }
                );
                assert_eq!(presentation.get(), "B");
                assert_eq!(restore_rows.load(Ordering::SeqCst), 0);
                submit_ok
            },
            |_| PickerOwnerApplyResult::Stale { actual: 0 },
        );
        assert_eq!(
            disposition,
            PickerResubmitDisposition::Submitted { opened: submit_ok }
        );
        assert_eq!(presentation.get(), if submit_ok { "B" } else { "A" });
        let deferred = reset
            .claim_deferred_with(&serialization, || reserved.get())
            .expect("deferred restore/reset claim");
        assert_eq!(
            execute_system_quit_restore_reset_with(
                || PickerResetBegin::Claimed(deferred),
                |_| {
                    restore_rows.fetch_add(1, Ordering::SeqCst);
                    renderer_refreshes.fetch_add(1, Ordering::SeqCst);
                    events.borrow_mut().push("restore");
                    presentation.set("restored");
                    reset_mutations.fetch_add(1, Ordering::SeqCst);
                    events.borrow_mut().push("reset");
                    presentation.set("none");
                },
            ),
            SystemQuitRestoreResetDisposition::Applied
        );
        assert_eq!(restore_rows.load(Ordering::SeqCst), 1);
        assert_eq!(renderer_refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(reset_mutations.load(Ordering::SeqCst), 1);
        assert_eq!(presentation.get(), "none");
        assert_eq!(
            events.into_inner(),
            if submit_ok {
                vec!["stage-B", "commit-B", "restore", "reset"]
            } else {
                vec!["stage-B", "rollback-A", "restore", "reset"]
            }
        );
    }

    let reset = PickerResetTransactionCoordinator::default();
    let serialization = std::sync::Mutex::new(());
    let reserved = std::cell::Cell::new(false);
    let restores = AtomicUsize::new(0);
    assert_eq!(
        execute_system_quit_restore_reset_with(
            || reset.begin_with(&serialization, || reserved.get()),
            |_| {
                assert!(!try_test_reservation(&reset, &serialization, &reserved));
                restores.fetch_add(1, Ordering::SeqCst);
            },
        ),
        SystemQuitRestoreResetDisposition::Applied
    );
    assert_eq!(restores.load(Ordering::SeqCst), 1);
    assert!(try_test_reservation(&reset, &serialization, &reserved));
}

#[test]
fn system_dialog_final_submit_lease_rejects_prior_zero_new_and_same_address_aba() {
    for replacement in [0, 0x6000, 0x5000] {
        let coordinator = PickerSystemDialogCoordinator::default();
        let published = AtomicUsize::new(0);
        let applied = AtomicUsize::new(0);
        let identity = match coordinator.publish_with(0x5000, |dialog| {
            apply_test_system_dialog(&published, &applied, dialog)
        }) {
            PickerSystemDialogPublicationDisposition::Published(identity) => identity,
            disposition => panic!("unexpected seed disposition: {disposition:?}"),
        };
        assert!(coordinator.begin_lease(identity));
        // This callback is the validation-to-submit race: publication wins before the submit
        // linearization point, so every replacement rejects with zero native calls.
        assert_eq!(
            coordinator.publish_with(replacement, |dialog| {
                apply_test_system_dialog(&published, &applied, dialog)
            }),
            PickerSystemDialogPublicationDisposition::Deferred
        );
        assert!(!coordinator.begin_submit(identity));
        let native_submits = AtomicUsize::new(0);
        assert_eq!(native_submits.load(Ordering::SeqCst), 0);
        coordinator
            .release_lease_with(|dialog| apply_test_system_dialog(&published, &applied, dialog));
        assert_eq!(published.load(Ordering::SeqCst), replacement);
        assert_eq!(applied.load(Ordering::SeqCst), 2);
        if replacement == 0 {
            assert!(coordinator.current_identity().is_none());
        } else {
            let newer = coordinator.current_identity().unwrap();
            assert_eq!(newer.dialog, replacement);
            assert_ne!(newer.generation, identity.generation);
            assert!(!coordinator.begin_lease(identity));
        }
    }
}

#[test]
fn owner_resubmit_final_system_identity_callback_rejects_zero_new_and_same_address_aba() {
    for replacement in [0, 0x6000, 0x5000] {
        let owner_coordinator = PickerOwnerLifetimeCoordinator::default();
        let system_coordinator = PickerSystemDialogCoordinator::default();
        let published = AtomicUsize::new(0);
        let applied = AtomicUsize::new(0);
        let identity = match system_coordinator.publish_with(0x5000, |dialog| {
            apply_test_system_dialog(&published, &applied, dialog)
        }) {
            PickerSystemDialogPublicationDisposition::Published(identity) => identity,
            disposition => panic!("unexpected seed disposition: {disposition:?}"),
        };
        assert!(system_coordinator.begin_lease(identity));
        let stages = AtomicUsize::new(0);
        let retry_latch = AtomicUsize::new(1);
        let releases = AtomicUsize::new(0);
        let rollbacks = AtomicUsize::new(0);
        let submits = AtomicUsize::new(0);
        assert_eq!(
            execute_owner_zero_resubmit_transaction_on_coordinator_with(
                &owner_coordinator,
                || true,
                || Some(()),
                || {
                    stages.fetch_add(1, Ordering::SeqCst);
                    true
                },
                || {
                    assert_eq!(
                        system_coordinator.publish_with(replacement, |dialog| {
                            apply_test_system_dialog(&published, &applied, dialog)
                        }),
                        PickerSystemDialogPublicationDisposition::Deferred
                    );
                    system_coordinator.begin_submit(identity)
                },
                || system_coordinator.cancel_submit(),
                |_| panic!("System identity loss must precede latch commit"),
                |_| {
                    releases.fetch_add(1, Ordering::SeqCst);
                },
                || {
                    rollbacks.fetch_add(1, Ordering::SeqCst);
                },
                || panic!("System identity loss must not commit staged rows"),
                || {
                    submits.fetch_add(1, Ordering::SeqCst);
                    true
                },
                |_| PickerOwnerApplyResult::Stale { actual: 0 },
            ),
            PickerResubmitDisposition::AuthorizationLost
        );
        system_coordinator
            .release_lease_with(|dialog| apply_test_system_dialog(&published, &applied, dialog));
        assert_eq!(stages.load(Ordering::SeqCst), 1);
        assert_eq!(retry_latch.load(Ordering::SeqCst), 1);
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        assert_eq!(rollbacks.load(Ordering::SeqCst), 1);
        assert_eq!(submits.load(Ordering::SeqCst), 0);
        assert_eq!(published.load(Ordering::SeqCst), replacement);
    }
}

#[test]
fn destination_resubmit_rejects_system_reset_before_latch_clear_and_submit() {
    let coordinator = PickerSystemDialogCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let identity = match coordinator.publish_with(0x5000, |dialog| {
        apply_test_system_dialog(&published, &applied, dialog)
    }) {
        PickerSystemDialogPublicationDisposition::Published(identity) => identity,
        disposition => panic!("unexpected seed disposition: {disposition:?}"),
    };
    let clears = AtomicUsize::new(0);
    let submits = AtomicUsize::new(0);
    let retry_latch = AtomicUsize::new(1);
    let reserved = std::cell::Cell::new(false);
    assert_eq!(
        execute_picker_destination_resubmit_on_coordinator_with(
            &coordinator,
            0,
            identity,
            || {
                if retry_latch.load(Ordering::SeqCst) != 1 || reserved.replace(true) {
                    None
                } else {
                    Some(())
                }
            },
            |_| reserved.set(false),
            |_| {
                assert!(reserved.replace(false));
                retry_latch.store(0, Ordering::SeqCst);
                clears.fetch_add(1, Ordering::SeqCst);
            },
            || {
                assert_eq!(
                    coordinator.publish_with(0, |dialog| {
                        apply_test_system_dialog(&published, &applied, dialog)
                    }),
                    PickerSystemDialogPublicationDisposition::Deferred
                );
                true
            },
            || {
                clears.fetch_add(1, Ordering::SeqCst);
            },
            || {
                submits.fetch_add(1, Ordering::SeqCst);
                true
            },
            |dialog| apply_test_system_dialog(&published, &applied, dialog),
        ),
        PickerResubmitDisposition::AuthorizationLost
    );
    assert_eq!(clears.load(Ordering::SeqCst), 0);
    assert_eq!(retry_latch.load(Ordering::SeqCst), 1);
    assert!(!reserved.get());
    assert_eq!(submits.load(Ordering::SeqCst), 0);
    assert_eq!(published.load(Ordering::SeqCst), 0);
}

#[test]
fn destination_false_submit_preserves_open_slots_then_exact_retry_clears_once() {
    let coordinator = PickerSystemDialogCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let identity = match coordinator.publish_with(0x5000, |dialog| {
        apply_test_system_dialog(&published, &applied, dialog)
    }) {
        PickerSystemDialogPublicationDisposition::Published(identity) => identity,
        disposition => panic!("unexpected seed disposition: {disposition:?}"),
    };
    let reopen = AtomicUsize::new(0);
    let open_slots = AtomicUsize::new(1);
    let destination = std::sync::Mutex::new(PickerDestinationResubmitState::default());
    let attempts = AtomicUsize::new(0);
    let successes = AtomicUsize::new(0);
    let commits = AtomicUsize::new(0);
    let run = |submit_ok: bool| {
        execute_picker_destination_resubmit_on_coordinator_with(
            &coordinator,
            0,
            identity,
            || {
                reserve_picker_destination_resubmit_transition_with(
                    &destination,
                    &reopen,
                    &open_slots,
                    identity,
                )
            },
            |reservation| {
                assert!(release_picker_destination_resubmit_reservation_with(
                    &destination,
                    reservation,
                ));
            },
            |reservation| {
                commit_picker_destination_resubmit_reservation_with(
                    &destination,
                    &reopen,
                    &open_slots,
                    reservation,
                );
                commits.fetch_add(1, Ordering::SeqCst);
            },
            || true,
            || {
                assert!(destination.lock().unwrap().reservation.is_some());
                assert_eq!(open_slots.load(Ordering::SeqCst), 1);
            },
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                if submit_ok {
                    successes.fetch_add(1, Ordering::SeqCst);
                }
                submit_ok
            },
            |dialog| apply_test_system_dialog(&published, &applied, dialog),
        )
    };

    assert_eq!(
        run(false),
        PickerResubmitDisposition::Submitted { opened: false }
    );
    assert_eq!(open_slots.load(Ordering::SeqCst), 1);
    assert!(destination.lock().unwrap().reservation.is_none());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(successes.load(Ordering::SeqCst), 0);
    assert_eq!(commits.load(Ordering::SeqCst), 0);

    assert_eq!(
        run(true),
        PickerResubmitDisposition::Submitted { opened: true }
    );
    assert_eq!(open_slots.load(Ordering::SeqCst), 0);
    assert!(destination.lock().unwrap().reservation.is_none());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(successes.load(Ordering::SeqCst), 1);
    assert_eq!(commits.load(Ordering::SeqCst), 1);
}

#[test]
fn system_dialog_reset_after_submit_linearization_defers_without_deadlock() {
    let coordinator = PickerSystemDialogCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let identity = match coordinator.publish_with(0x5000, |dialog| {
        apply_test_system_dialog(&published, &applied, dialog)
    }) {
        PickerSystemDialogPublicationDisposition::Published(identity) => identity,
        disposition => panic!("unexpected seed disposition: {disposition:?}"),
    };
    assert!(coordinator.begin_lease(identity));
    assert!(coordinator.begin_submit(identity));
    let submits = AtomicUsize::new(0);
    assert_eq!(
        coordinator.publish_with(0, |dialog| {
            apply_test_system_dialog(&published, &applied, dialog)
        }),
        PickerSystemDialogPublicationDisposition::Deferred
    );
    submits.fetch_add(1, Ordering::SeqCst);
    assert_eq!(published.load(Ordering::SeqCst), 0x5000);
    coordinator.release_lease_with(|dialog| apply_test_system_dialog(&published, &applied, dialog));
    assert_eq!(submits.load(Ordering::SeqCst), 1);
    assert_eq!(published.load(Ordering::SeqCst), 0);
    assert!(coordinator.current_identity().is_none());
}

fn seed_test_live_token(
    coordinator: &PickerOwnerLifetimeCoordinator,
    published: &AtomicUsize,
    applied: &AtomicUsize,
    dialog: usize,
    job: usize,
    vtable: usize,
) -> PickerProfileRunToken {
    let _ = coordinator.publish_with(
        PickerOwnerPublicationRequest::Set {
            new_dialog: dialog,
            job,
        },
        |request| apply_test_owner_publication(published, applied, request),
    );
    let list = job + 0x100;
    let run = coordinator
        .register_live_run(job, dialog, list)
        .expect("exact current Run registration");
    PickerProfileRunToken {
        job,
        list,
        dialog,
        owner_generation: run.owner_generation,
        job_lineage: run.job_lineage,
        run_lineage: run.run_lineage,
        observed_vtable: vtable,
        expected_vtable: vtable,
    }
}

fn seed_test_owner_cleared_lineage(
    coordinator: &PickerOwnerLifetimeCoordinator,
    published: &AtomicUsize,
    applied: &AtomicUsize,
    dialog: usize,
    job: usize,
) -> PickerOwnerClearedLineage {
    let _ = seed_test_live_token(coordinator, published, applied, dialog, job, 0x142b229f8);
    let _ = coordinator.publish_with(
        PickerOwnerPublicationRequest::CompareSet {
            expected: dialog,
            new_dialog: 0,
        },
        |request| apply_test_owner_publication(published, applied, request),
    );
    coordinator
        .cleared_lineage_for_job(job)
        .expect("exact cleared owner lineage")
}

fn owner_cleared_observation(
    old_dialog: usize,
    job: usize,
    owner_generation: usize,
    resubmit_generation: usize,
) -> PickerProfileRunObservation {
    PickerProfileRunObservation::OwnerCleared(PickerOwnerClearedAuthority {
        observed_job: job,
        lineage: PickerOwnerClearedLineage {
            old_owner: PickerOwnerLineage {
                dialog: old_dialog,
                generation: owner_generation,
                job,
                job_lineage: owner_generation,
            },
            old_run: PickerRunRegistration {
                owner_generation,
                job,
                list: job + 0x100,
                job_lineage: owner_generation,
                run_lineage: owner_generation,
            },
            zero_generation: owner_generation,
        },
        pending: PickerPendingResubmitTransition {
            old_dialog,
            system_dialog: 0x7000,
            system_dialog_generation: owner_generation,
            path_owner_generation: owner_generation,
            refresh_owner_generation: 0,
            refresh_close_generation: 0,
            reopen_pending: 1,
            open_slots_pending: 0,
            resubmit_generation,
        },
    })
}

#[test]
fn stage_cas_resets_ticks_only_when_expected_stage_matches() {
    let stage = AtomicUsize::new(3);
    let ticks = AtomicUsize::new(41);

    assert_eq!(save_flow_menu_stage_cas(&stage, &ticks, 3, 8), Ok(3));
    assert_eq!(stage.load(Ordering::SeqCst), 8);
    assert_eq!(ticks.load(Ordering::SeqCst), 0);
}

#[test]
fn stage_cas_refuses_stale_menu_thread_decisions() {
    let stage = AtomicUsize::new(9);
    let ticks = AtomicUsize::new(41);

    assert_eq!(save_flow_menu_stage_cas(&stage, &ticks, 3, 8), Err(9));
    assert_eq!(stage.load(Ordering::SeqCst), 9);
    assert_eq!(ticks.load(Ordering::SeqCst), 41);
}

struct TestRefreshState {
    pending_dialog: AtomicUsize,
    pending_generation: AtomicUsize,
    reopen: AtomicUsize,
    generations: AtomicUsize,
}

impl TestRefreshState {
    fn new() -> Self {
        Self {
            pending_dialog: AtomicUsize::new(0),
            pending_generation: AtomicUsize::new(0),
            reopen: AtomicUsize::new(0),
            generations: AtomicUsize::new(0),
        }
    }

    fn queue(&self, dialog: usize) -> PickerRefreshRequestDisposition {
        queue_picker_refresh_request_with(
            &self.pending_dialog,
            &self.pending_generation,
            self.reopen.load(Ordering::SeqCst) != 0,
            dialog,
            || self.generations.fetch_add(1, Ordering::SeqCst) + 1,
        )
    }

    fn request(&self) -> PickerRefreshRequest {
        load_picker_refresh_request_with(&self.pending_dialog, &self.pending_generation)
            .expect("exact pending refresh request")
    }

    fn retire(&self, request: PickerRefreshRequest, keep_reopen: bool) -> bool {
        retire_picker_refresh_request_with(
            &self.pending_dialog,
            &self.pending_generation,
            &self.reopen,
            request,
            keep_reopen,
        )
    }

    fn assert_request(&self, request: PickerRefreshRequest) {
        assert_eq!(self.request(), request);
    }

    fn assert_cleared(&self, reopen: usize) {
        assert_eq!(self.pending_dialog.load(Ordering::SeqCst), 0);
        assert_eq!(self.pending_generation.load(Ordering::SeqCst), 0);
        assert_eq!(self.reopen.load(Ordering::SeqCst), reopen);
    }
}

#[test]
fn profile_load_vtable_expectation_is_the_shared_1162_rva() {
    assert_eq!(PROFILE_LOAD_DIALOG_VTABLE_RVA, 0x2b229f8);
    assert_eq!(
        0x140000000_usize + PROFILE_LOAD_DIALOG_VTABLE_RVA,
        0x142b229f8
    );
}

fn live_token(dialog: usize) -> PickerProfileRunToken {
    PickerProfileRunToken {
        job: 0x4100,
        list: 0x4200,
        dialog,
        owner_generation: 1,
        job_lineage: 1,
        run_lineage: 1,
        observed_vtable: 0x6000,
        expected_vtable: 0x6000,
    }
}

fn live_run_registration(job: usize) -> PickerRunRegistration {
    PickerRunRegistration {
        owner_generation: 1,
        job,
        list: job + 0x100,
        job_lineage: 1,
        run_lineage: 1,
    }
}

fn live_observation(dialog: usize) -> PickerProfileRunObservation {
    PickerProfileRunObservation::Live(live_token(dialog))
}

fn ticket(dialog: usize) -> er_save_picker::PathEditorDeferredCloseTicket {
    er_save_picker::PathEditorDeferredCloseTicket {
        dialog,
        generation: 91,
        owner: er_save_picker::PathEditorPickerIdentity {
            picker_mode_active: true,
            current_dialog: dialog,
        },
    }
}

#[test]
fn repeated_refresh_requests_for_one_owner_coalesce_one_exact_generation() {
    let state = TestRefreshState::new();
    let first = state.queue(0x5000);
    let request = match first {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("first request must queue, got {other:?}"),
    };
    assert_eq!(
        state.queue(0x5000),
        PickerRefreshRequestDisposition::Coalesced(Some(request))
    );
    state.assert_request(request);
    assert_eq!(state.generations.load(Ordering::SeqCst), 1);
}

#[test]
fn no_close_return_reopen_cannot_coalesce_away_changed_content_generation() {
    let state = TestRefreshState::new();
    state.reopen.store(1, Ordering::SeqCst);
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("changed content must queue behind return reopen, got {other:?}"),
    };
    state.assert_request(request);
    assert_eq!(state.generations.load(Ordering::SeqCst), 1);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 1);
}

#[test]
fn immediate_close_retains_exact_request_until_owner_zero_claim() {
    let state = TestRefreshState::new();
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("first request must queue, got {other:?}"),
    };
    let disposition = consume_picker_refresh_with(
        request,
        request.dialog,
        true,
        false,
        false,
        false,
        live_observation(request.dialog),
        || state.reopen.store(1, Ordering::SeqCst),
        |_| PickerRefreshCloseDisposition::Closed,
    );
    assert_eq!(
        disposition,
        PickerRefreshConsumeDisposition::CloseRequested(PickerRefreshCloseDisposition::Closed)
    );
    let PickerRefreshConsumeDisposition::CloseRequested(close) = disposition else {
        unreachable!()
    };
    assert_eq!(
        apply_picker_refresh_close_with(request, close, |request, keep| {
            state.retire(request, keep)
        }),
        PickerRefreshCloseResolution::Retained
    );
    state.assert_request(request);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 1);
    assert!(picker_resubmit_pending_with(
        state.reopen.load(Ordering::SeqCst),
        0
    ));
}

#[test]
fn deferred_close_retains_exact_request_until_retry_drains() {
    let state = TestRefreshState::new();
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("first request must queue, got {other:?}"),
    };
    let deferred = PickerRefreshCloseDisposition::Deferred(ticket(request.dialog));
    let disposition = consume_picker_refresh_with(
        request,
        request.dialog,
        true,
        false,
        false,
        false,
        live_observation(request.dialog),
        || state.reopen.store(1, Ordering::SeqCst),
        |_| deferred,
    );
    assert_eq!(
        disposition,
        PickerRefreshConsumeDisposition::CloseRequested(deferred)
    );
    assert_eq!(
        apply_picker_refresh_close_with(request, deferred, |request, keep| {
            state.retire(request, keep)
        }),
        PickerRefreshCloseResolution::Retained
    );
    state.assert_request(request);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 1);

    assert_eq!(
        apply_picker_refresh_retry_with(
            Some(request),
            PickerRefreshRetryOutcome::DrainedClosed {
                dialog: request.dialog,
            },
            |request, keep| state.retire(request, keep),
        ),
        PickerRefreshRetryResolution::Retained
    );
    state.assert_request(request);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 1);
}

#[test]
fn reset_in_progress_retains_then_retries_the_exact_request() {
    let state = TestRefreshState::new();
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("first request must queue, got {other:?}"),
    };
    let first = consume_picker_refresh_with(
        request,
        request.dialog,
        true,
        false,
        false,
        false,
        live_observation(request.dialog),
        || state.reopen.store(1, Ordering::SeqCst),
        |_| PickerRefreshCloseDisposition::ResetInProgress,
    );
    let PickerRefreshConsumeDisposition::CloseRequested(first_close) = first else {
        panic!("reset race must reach explicit close disposition")
    };
    assert_eq!(
        apply_picker_refresh_close_with(request, first_close, |request, keep| {
            state.retire(request, keep)
        }),
        PickerRefreshCloseResolution::Retained
    );
    state.assert_request(request);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 1);

    let retry = consume_picker_refresh_with(
        request,
        request.dialog,
        true,
        false,
        false,
        false,
        live_observation(request.dialog),
        || state.reopen.store(1, Ordering::SeqCst),
        |_| PickerRefreshCloseDisposition::Closed,
    );
    let PickerRefreshConsumeDisposition::CloseRequested(retry_close) = retry else {
        panic!("request must retry after reset lease clears")
    };
    assert_eq!(
        apply_picker_refresh_close_with(request, retry_close, |request, keep| {
            state.retire(request, keep)
        }),
        PickerRefreshCloseResolution::Retained
    );
    state.assert_request(request);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 1);
}

#[test]
fn cancelled_and_resolve_failed_close_clear_exact_refresh_without_resubmit() {
    for close in [
        PickerRefreshCloseDisposition::Cancelled(Some(ticket(0x5000))),
        PickerRefreshCloseDisposition::ResolveFailed,
        PickerRefreshCloseDisposition::Rejected,
    ] {
        let state = TestRefreshState::new();
        let request = match state.queue(0x5000) {
            PickerRefreshRequestDisposition::Queued(request) => request,
            other => panic!("first request must queue, got {other:?}"),
        };
        state.reopen.store(1, Ordering::SeqCst);
        assert_eq!(
            apply_picker_refresh_close_with(request, close, |request, keep| {
                state.retire(request, keep)
            }),
            PickerRefreshCloseResolution::RetiredClearReopen
        );
        state.assert_cleared(0);
        assert!(!picker_resubmit_pending_with(
            state.reopen.load(Ordering::SeqCst),
            0
        ));
    }
}

#[test]
fn failed_or_cancelled_deferred_retry_clears_exact_refresh_without_resubmit() {
    for outcome in [
        PickerRefreshRetryOutcome::DrainedFailed { dialog: 0x5000 },
        PickerRefreshRetryOutcome::Cancelled { dialog: 0x5000 },
    ] {
        let state = TestRefreshState::new();
        let request = match state.queue(0x5000) {
            PickerRefreshRequestDisposition::Queued(request) => request,
            other => panic!("first request must queue, got {other:?}"),
        };
        state.reopen.store(1, Ordering::SeqCst);
        assert_eq!(
            apply_picker_refresh_retry_with(Some(request), outcome, |request, keep| {
                state.retire(request, keep)
            }),
            PickerRefreshRetryResolution::RetiredClearReopen
        );
        state.assert_cleared(0);
        assert!(!picker_resubmit_pending_with(0, 0));
    }
}

#[test]
fn stale_identity_precedes_already_closing_and_never_dereferences_old_dialog() {
    let state = TestRefreshState::new();
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("first request must queue, got {other:?}"),
    };
    state.reopen.store(1, Ordering::SeqCst);
    assert_eq!(
        consume_picker_refresh_with(
            request,
            0x6000,
            true,
            false,
            true,
            false,
            PickerProfileRunObservation::OtherResource,
            || panic!("stale request must not arm reopen"),
            |_| panic!("stale request must not dereference old dialog"),
        ),
        PickerRefreshConsumeDisposition::StaleIdentity
    );
    assert!(state.retire(request, false));
    state.assert_cleared(0);
    assert!(!picker_resubmit_pending_with(0, 0));
}

#[test]
fn modal_refresh_defers_without_arming_or_closing() {
    let state = TestRefreshState::new();
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("first request must queue, got {other:?}"),
    };
    assert_eq!(
        consume_picker_refresh_with(
            request,
            request.dialog,
            true,
            true,
            false,
            false,
            live_observation(request.dialog),
            || panic!("modal deferral must not arm reopen"),
            |_| panic!("modal deferral must not close"),
        ),
        PickerRefreshConsumeDisposition::Deferred
    );
    state.assert_request(request);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 0);
}

#[test]
fn stage_boundary_accepts_both_owner_zero_sites_and_rejects_live_owner() {
    assert!(
        save_picker_stage_owner_is_clear(0),
        "initial open owner-zero"
    );
    assert!(save_picker_stage_owner_is_clear(0), "resubmit owner-zero");
    assert!(!save_picker_stage_owner_is_clear(0x5000));
}

#[test]
fn destination_resubmit_waits_for_owner_zero_and_final_authority() {
    let submits = AtomicUsize::new(0);
    assert_eq!(
        execute_picker_resubmit_with(
            0x5000,
            || true,
            || panic!("live owner must not clear destination latch"),
            || panic!("live owner must not submit"),
        ),
        PickerResubmitDisposition::WaitingForOwnerClear
    );
    assert_eq!(
        execute_picker_resubmit_with(
            0,
            || false,
            || panic!("lost authority must not clear destination latch"),
            || panic!("lost authority must not submit"),
        ),
        PickerResubmitDisposition::AuthorizationLost
    );
    assert_eq!(
        execute_picker_resubmit_with(
            0,
            || true,
            || {},
            || {
                submits.fetch_add(1, Ordering::SeqCst);
                true
            },
        ),
        PickerResubmitDisposition::Submitted { opened: true }
    );
    assert_eq!(submits.load(Ordering::SeqCst), 1);
}

#[test]
fn outer_post_permissions_fail_closed_for_generic_02_990_wrong_and_live_resources() {
    let generic = PickerOuterPostAuthority::Other;
    let native_calls = AtomicUsize::new(0);
    for authority in [generic, PickerOuterPostAuthority::Other] {
        let permissions = picker_outer_post_permissions_with(authority, true, true, true);
        assert_eq!(
            permissions,
            PickerOuterPostPermissions {
                destination_open: false,
                live_profile_token: None,
                picker_submit: false,
            }
        );
        assert!(
            permissions
                .run_destination(|| native_calls.fetch_add(1, Ordering::SeqCst))
                .is_none()
        );
        assert!(
            permissions
                .run_live_profile(|_| native_calls.fetch_add(1, Ordering::SeqCst))
                .is_none()
        );
        assert!(
            permissions
                .run_picker_submit(|| native_calls.fetch_add(1, Ordering::SeqCst))
                .is_none()
        );
    }
    assert_eq!(native_calls.load(Ordering::SeqCst), 0);
    assert!(
        observe_picker_destination_parent_with(false, 0x4100, 0x5000, 0x6000, 0x5000).is_none()
    );
    assert!(observe_picker_destination_parent_with(true, 0x4100, 0x5000, 0x6000, 0x6000).is_none());

    let parent = PickerOuterPostAuthority::DestinationParent(
        observe_picker_destination_parent_with(true, 0x4100, 0x5000, 0x6000, 0x5000)
            .expect("exact options parent"),
    );
    let parent_permissions = picker_outer_post_permissions_with(parent, true, true, false);
    assert_eq!(
        parent_permissions,
        PickerOuterPostPermissions {
            destination_open: true,
            live_profile_token: None,
            picker_submit: true,
        }
    );
    assert!(
        parent_permissions
            .run_destination(|| native_calls.fetch_add(1, Ordering::SeqCst))
            .is_some()
    );
    assert!(
        parent_permissions
            .run_picker_submit(|| native_calls.fetch_add(1, Ordering::SeqCst))
            .is_some()
    );
    assert!(!picker_outer_post_permissions_with(parent, true, true, true).picker_submit);
    assert!(!picker_outer_authority_still_current_with(
        parent,
        || 0,
        || 0x6000,
        |_| true,
        |_| false,
        |_| panic!("changed parent rejects before vtable read"),
    ));
    assert!(picker_outer_authority_still_current_with(
        parent,
        || 0,
        || 0x5000,
        |_| true,
        |_| false,
        |_| Some(0x6000),
    ));

    let live = PickerOuterPostAuthority::Profile(live_observation(0x5000));
    let live_permissions = picker_outer_post_permissions_with(live, true, true, true);
    assert!(!live_permissions.destination_open);
    assert_eq!(
        live_permissions.live_profile_token,
        Some(live_token(0x5000))
    );
    assert!(!live_permissions.picker_submit);
    assert!(
        live_permissions
            .run_live_profile(|_| native_calls.fetch_add(1, Ordering::SeqCst))
            .is_some()
    );

    let owner_zero =
        PickerOuterPostAuthority::Profile(owner_cleared_observation(0x5000, 0x4100, 7, 9));
    let owner_zero_permissions = picker_outer_post_permissions_with(owner_zero, true, false, true);
    assert!(!owner_zero_permissions.destination_open);
    assert!(owner_zero_permissions.live_profile_token.is_none());
    assert!(owner_zero_permissions.picker_submit);
    assert!(
        owner_zero_permissions
            .run_picker_submit(|| native_calls.fetch_add(1, Ordering::SeqCst))
            .is_some()
    );
    assert_eq!(native_calls.load(Ordering::SeqCst), 4);
    assert!(!picker_outer_authority_still_current_with(
        owner_zero,
        || 0,
        || 0,
        |_| true,
        |_| false,
        |_| None,
    ));
    assert!(!picker_outer_authority_still_current_with(
        owner_zero,
        || 0x7000,
        || 0,
        |_| true,
        |_| false,
        |_| None,
    ));
}

#[test]
fn generic_and_02_990_posts_cannot_run_any_native_picker_maintenance() {
    let pointer_free = AtomicUsize::new(0);
    let queue_ready = AtomicUsize::new(0);
    let constructor = AtomicUsize::new(0);
    let submit = AtomicUsize::new(0);
    let other_native = AtomicUsize::new(0);
    for observation in [
        PickerProfileRunObservation::OtherResource,
        observe_picker_profile_run_with(
            false, 0x4100, 0x4200, 0x5000, 0x6000, 0x5000, 0x6000, None,
        ),
    ] {
        pump_picker_native_maintenance_with(
            observation,
            || {
                pointer_free.fetch_add(1, Ordering::SeqCst);
            },
            |_| {
                queue_ready.fetch_add(1, Ordering::SeqCst);
                constructor.fetch_add(1, Ordering::SeqCst);
                submit.fetch_add(1, Ordering::SeqCst);
            },
            |_| {
                other_native.fetch_add(1, Ordering::SeqCst);
            },
            |_| {
                other_native.fetch_add(1, Ordering::SeqCst);
            },
            |_| {
                other_native.fetch_add(1, Ordering::SeqCst);
            },
        );
    }
    assert_eq!(pointer_free.load(Ordering::SeqCst), 2);
    assert_eq!(queue_ready.load(Ordering::SeqCst), 0);
    assert_eq!(constructor.load(Ordering::SeqCst), 0);
    assert_eq!(submit.load(Ordering::SeqCst), 0);
    assert_eq!(other_native.load(Ordering::SeqCst), 0);
}

#[test]
fn exact_profile_run_token_allows_each_native_maintenance_sink_once() {
    let path = AtomicUsize::new(0);
    let native_path_submit = AtomicUsize::new(0);
    let drive = AtomicUsize::new(0);
    let scrollbar = AtomicUsize::new(0);
    let edge = AtomicUsize::new(0);
    pump_picker_native_maintenance_with(
        live_observation(0x5000),
        || {
            path.fetch_add(1, Ordering::SeqCst);
        },
        |token| {
            assert_eq!(token.dialog, 0x5000);
            native_path_submit.fetch_add(1, Ordering::SeqCst);
        },
        |token| {
            assert_eq!(token.dialog, 0x5000);
            drive.fetch_add(1, Ordering::SeqCst);
        },
        |_| {
            scrollbar.fetch_add(1, Ordering::SeqCst);
        },
        |_| {
            edge.fetch_add(1, Ordering::SeqCst);
        },
    );
    assert_eq!(path.load(Ordering::SeqCst), 1);
    assert_eq!(native_path_submit.load(Ordering::SeqCst), 1);
    assert_eq!(drive.load(Ordering::SeqCst), 1);
    assert_eq!(scrollbar.load(Ordering::SeqCst), 1);
    assert_eq!(edge.load(Ordering::SeqCst), 1);
}

#[test]
fn profile_run_observation_rejects_wrong_owner_and_vtable() {
    assert_eq!(
        observe_picker_profile_run_with(
            true,
            0x4100,
            0x4200,
            0x5000,
            0x6000,
            0x5000,
            0x6000,
            Some(live_run_registration(0x4100)),
        ),
        live_observation(0x5000)
    );
    assert!(matches!(
        observe_picker_profile_run_with(
            true,
            1,
            0x101,
            0x5008,
            0x6000,
            0x5000,
            0x6000,
            Some(live_run_registration(1)),
        ),
        PickerProfileRunObservation::Rejected { .. }
    ));
    assert!(matches!(
        observe_picker_profile_run_with(
            true,
            1,
            0x101,
            0x5000,
            0x7000,
            0x5000,
            0x6000,
            Some(live_run_registration(1)),
        ),
        PickerProfileRunObservation::Rejected { .. }
    ));
    assert_eq!(
        observe_picker_profile_run_with(false, 1, 0x101, 0x5000, 0x6000, 0x5000, 0x6000, None,),
        PickerProfileRunObservation::OtherResource
    );
}

#[test]
fn profile_run_observation_rejects_zero_job_even_with_exact_owner_vtable_and_registration() {
    assert!(matches!(
        observe_picker_profile_run_with(
            true,
            0,
            0x100,
            0x5000,
            0x6000,
            0x5000,
            0x6000,
            Some(PickerRunRegistration {
                owner_generation: 1,
                job: 0,
                list: 0x100,
                job_lineage: 1,
                run_lineage: 1,
            }),
        ),
        PickerProfileRunObservation::Rejected { job: 0, .. }
    ));
}

#[test]
fn exact_live_token_rejects_same_address_job_aba_and_accepts_current_job() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let token_a = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x4100,
        0x142b229f8,
    );
    let token_b = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x4200,
        0x142b229f8,
    );
    assert_ne!(token_a.owner_generation, token_b.owner_generation);
    assert_ne!(token_a.job_lineage, token_b.job_lineage);
    assert_eq!(
        execute_picker_live_token_call_on_coordinator_with(
            &coordinator,
            token_a,
            || published.load(Ordering::SeqCst),
            |_| Some(0x142b229f8),
            |_| calls.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        Err(PickerNativeCloseRejectReason::InvalidTokenLineage)
    );
    assert!(
        execute_picker_live_token_call_on_coordinator_with(
            &coordinator,
            token_b,
            || published.load(Ordering::SeqCst),
            |_| Some(0x142b229f8),
            |_| calls.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        )
        .is_ok()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn owner_zero_and_zero_job_publications_invalidate_the_previous_exact_token() {
    for replacement in [
        PickerOwnerPublicationRequest::CompareSet {
            expected: 0x5000,
            new_dialog: 0,
        },
        PickerOwnerPublicationRequest::Set {
            new_dialog: 0x5000,
            job: 0,
        },
    ] {
        let coordinator = PickerOwnerLifetimeCoordinator::default();
        let published = AtomicUsize::new(0);
        let applied = AtomicUsize::new(0);
        let calls = AtomicUsize::new(0);
        let token = seed_test_live_token(
            &coordinator,
            &published,
            &applied,
            0x5000,
            0x4100,
            0x142b229f8,
        );
        let _ = coordinator.publish_with(replacement, |request| {
            apply_test_owner_publication(&published, &applied, request)
        });
        assert_eq!(
            execute_picker_live_token_call_on_coordinator_with(
                &coordinator,
                token,
                || published.load(Ordering::SeqCst),
                |_| Some(0x142b229f8),
                |_| calls.fetch_add(1, Ordering::SeqCst),
                |request| apply_test_owner_publication(&published, &applied, request),
            ),
            Err(PickerNativeCloseRejectReason::InvalidTokenLineage)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn exact_live_token_rejects_older_run_nonce_of_same_job_and_owner() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let old = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x4100,
        0x142b229f8,
    );
    let newer_run = coordinator
        .register_live_run(0x4100, 0x5000, 0x4200)
        .expect("new exact Run nonce");
    let current = PickerProfileRunToken {
        run_lineage: newer_run.run_lineage,
        ..old
    };
    assert_ne!(old.run_lineage, current.run_lineage);
    assert_eq!(
        execute_picker_live_token_call_on_coordinator_with(
            &coordinator,
            old,
            || published.load(Ordering::SeqCst),
            |_| Some(0x142b229f8),
            |_| calls.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        Err(PickerNativeCloseRejectReason::InvalidTokenLineage)
    );
    assert!(
        execute_picker_live_token_call_on_coordinator_with(
            &coordinator,
            current,
            || published.load(Ordering::SeqCst),
            |_| Some(0x142b229f8),
            |_| calls.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        )
        .is_ok()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn token_last_moment_vtable_mismatch_is_not_current() {
    let token = live_token(0x5000);
    assert!(picker_profile_token_still_current_with(
        token,
        0x5000,
        |_| true,
        |_| Some(0x6000)
    ));
    assert!(!picker_profile_token_still_current_with(
        token,
        0x5000,
        |_| true,
        |_| Some(0x7000)
    ));
    assert!(!picker_profile_token_still_current_with(
        token,
        0x6000,
        |_| true,
        |_| panic!("wrong identity must reject before vtable read")
    ));
}

#[test]
fn common_exact_token_lease_rechecks_raw_owner_and_vtable_for_every_native_wrapper() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let calls = AtomicUsize::new(0);
    let token = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x4100,
        0x142b229f8,
    );
    assert_eq!(
        execute_picker_live_token_call_on_coordinator_with(
            &coordinator,
            token,
            || published.load(Ordering::SeqCst),
            |_| Some(0x142b22000),
            |_| calls.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        Err(PickerNativeCloseRejectReason::UnexpectedVtable)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    published.store(0x6000, Ordering::SeqCst);
    assert_eq!(
        execute_picker_live_token_call_on_coordinator_with(
            &coordinator,
            token,
            || published.load(Ordering::SeqCst),
            |_| panic!("changed raw owner rejects before vtable read"),
            |_| calls.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        Err(PickerNativeCloseRejectReason::PublishedOwnerMismatch)
    );
    published.store(0x5000, Ordering::SeqCst);
    for _sink in [
        "close",
        "event-cursor",
        "cursor",
        "focus",
        "scroll-total",
        "scroll-pos",
        "edge",
        "queue-ready",
        "ctor",
        "submit",
    ] {
        assert!(
            execute_picker_live_token_call_on_coordinator_with(
                &coordinator,
                token,
                || published.load(Ordering::SeqCst),
                |_| Some(0x142b229f8),
                |_| calls.fetch_add(1, Ordering::SeqCst),
                |request| apply_test_owner_publication(&published, &applied, request),
            )
            .is_ok()
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 10);
}

#[test]
fn owner_lifetime_lease_defers_reentrant_publication_from_vtable_validation_until_sink_returns() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let sinks = AtomicUsize::new(0);
    let token = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x4100,
        0x142b229f8,
    );
    applied.store(0, Ordering::SeqCst);

    let retire = PickerOwnerPublicationRequest::CompareSet {
        expected: 0x5000,
        new_dialog: 0,
    };
    let result = execute_picker_live_token_call_on_coordinator_with(
        &coordinator,
        token,
        || published.load(Ordering::SeqCst),
        |_| {
            // This is synchronous same-thread re-entry from final validation. It must return, not
            // deadlock, and duplicate publications must coalesce to one deferred application.
            assert!(
                coordinator
                    .register_live_run(0x4100, 0x5000, 0x4200)
                    .is_none()
            );
            for _ in 0..2 {
                assert_eq!(
                    coordinator.publish_with(retire, |request| {
                        apply_test_owner_publication(&published, &applied, request)
                    }),
                    PickerOwnerPublicationDisposition::Deferred
                );
            }
            assert_eq!(published.load(Ordering::SeqCst), 0x5000);
            assert_eq!(coordinator.snapshot_for_test().0, 1);
            Some(0x142b229f8)
        },
        |dialog| {
            assert_eq!(dialog, 0x5000);
            assert_eq!(published.load(Ordering::SeqCst), 0x5000);
            assert_eq!(
                coordinator.publish_with(retire, |request| {
                    apply_test_owner_publication(&published, &applied, request)
                }),
                PickerOwnerPublicationDisposition::Deferred
            );
            sinks.fetch_add(1, Ordering::SeqCst);
        },
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(result, Ok((0x5000, 0x142b229f8, ())));
    assert_eq!(sinks.load(Ordering::SeqCst), 1);
    assert_eq!(published.load(Ordering::SeqCst), 0);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
    let snapshot = coordinator.snapshot_for_test();
    assert_eq!(snapshot.0, 0);
    assert!(snapshot.1.is_empty());
    assert!(snapshot.2.is_none());
    assert_eq!(snapshot.3.unwrap().old_owner.dialog, 0x5000);
}

#[test]
fn owner_lifetime_lease_rejects_bad_vtable_then_applies_deferred_publication_once() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let sinks = AtomicUsize::new(0);
    let token = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x4100,
        0x142b229f8,
    );
    applied.store(0, Ordering::SeqCst);
    let retire = PickerOwnerPublicationRequest::CompareSet {
        expected: 0x5000,
        new_dialog: 0,
    };
    assert_eq!(
        execute_picker_live_token_call_on_coordinator_with(
            &coordinator,
            token,
            || published.load(Ordering::SeqCst),
            |_| {
                assert_eq!(
                    coordinator.publish_with(retire, |request| {
                        apply_test_owner_publication(&published, &applied, request)
                    }),
                    PickerOwnerPublicationDisposition::Deferred
                );
                Some(0x142b22000)
            },
            |_| {
                sinks.fetch_add(1, Ordering::SeqCst);
            },
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        Err(PickerNativeCloseRejectReason::UnexpectedVtable)
    );
    assert_eq!(sinks.load(Ordering::SeqCst), 0);
    assert_eq!(published.load(Ordering::SeqCst), 0);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
}

#[test]
fn owner_cleared_authority_rejects_old_pending_generation_then_exact_retry_succeeds_once() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let lineage =
        seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4100);
    let pending_a = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 7,
        refresh_owner_generation: 0,
        refresh_close_generation: 0,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let pending_b = PickerPendingResubmitTransition {
        resubmit_generation: 11,
        ..pending_a
    };
    let current_pending = std::cell::Cell::new(Some(pending_b));
    let latches = AtomicUsize::new(1);
    let submits = AtomicUsize::new(0);
    let old_authority = PickerOwnerClearedAuthority {
        observed_job: 0x4100,
        lineage,
        pending: pending_a,
    };
    assert!(
        execute_owner_zero_submit_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    old_authority,
                    coordinator.cleared_lineage_for_job(0x4100),
                    current_pending.get(),
                )
            },
            || panic!("stale pending generation must not clear latches"),
            || submits.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        )
        .is_none()
    );
    assert_eq!(latches.load(Ordering::SeqCst), 1);
    assert_eq!(submits.load(Ordering::SeqCst), 0);

    let exact_authority = PickerOwnerClearedAuthority {
        pending: pending_b,
        ..old_authority
    };
    assert_eq!(
        execute_owner_zero_submit_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    exact_authority,
                    coordinator.cleared_lineage_for_job(0x4100),
                    current_pending.get(),
                )
            },
            || {
                if current_pending.get() != Some(pending_b) {
                    return false;
                }
                current_pending.set(None);
                latches.store(0, Ordering::SeqCst);
                true
            },
            || {
                assert_eq!(published.load(Ordering::SeqCst), 0);
                assert_eq!(
                    coordinator.publish_with(
                        PickerOwnerPublicationRequest::Set {
                            new_dialog: 0x5000,
                            job: 0x4200,
                        },
                        |request| apply_test_owner_publication(&published, &applied, request),
                    ),
                    PickerOwnerPublicationDisposition::Deferred
                );
                submits.fetch_add(1, Ordering::SeqCst)
            },
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        Some(0)
    );
    assert_eq!(published.load(Ordering::SeqCst), 0x5000);
    assert_eq!(latches.load(Ordering::SeqCst), 0);
    assert_eq!(submits.load(Ordering::SeqCst), 1);
    assert!(
        execute_owner_zero_submit_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    exact_authority,
                    coordinator.cleared_lineage_for_job(0x4100),
                    current_pending.get(),
                )
            },
            || true,
            || submits.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        )
        .is_none()
    );
    assert_eq!(submits.load(Ordering::SeqCst), 1);
}

#[test]
fn owner_cleared_authority_rejects_same_address_aba_and_wrong_job() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let old_lineage =
        seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4100);
    let pending = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 7,
        refresh_owner_generation: 0,
        refresh_close_generation: 0,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let old_authority = PickerOwnerClearedAuthority {
        observed_job: 0x4100,
        lineage: old_lineage,
        pending,
    };
    let new_lineage =
        seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4200);
    assert_ne!(old_lineage, new_lineage);
    assert!(!picker_owner_cleared_authority_matches(
        old_authority,
        coordinator.cleared_lineage_for_job(0x4100),
        Some(pending)
    ));
    let wrong_job = PickerOwnerClearedAuthority {
        observed_job: 0x4300,
        lineage: new_lineage,
        pending,
    };
    assert!(!picker_owner_cleared_authority_matches(
        wrong_job,
        coordinator.cleared_lineage_for_job(0x4200),
        Some(pending)
    ));
}

#[test]
fn exact_pending_transition_claim_preserves_new_generation_then_clears_only_exact_latches() {
    let pending_a = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 7,
        refresh_owner_generation: 8,
        refresh_close_generation: 0,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let pending_b = PickerPendingResubmitTransition {
        path_owner_generation: 9,
        refresh_owner_generation: 10,
        resubmit_generation: 11,
        ..pending_a
    };
    let state = std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 11,
        pending: Some(pending_b),
        ..PickerPendingResubmitState::default()
    });
    let path_dialog = AtomicUsize::new(0x5000);
    let path_generation = AtomicUsize::new(9);
    let refresh_dialog = AtomicUsize::new(0x5000);
    let refresh_generation = AtomicUsize::new(10);
    let refresh_close_generation = AtomicUsize::new(0);
    let reopen = AtomicUsize::new(1);
    let open_slots = AtomicUsize::new(0);
    assert!(!claim_picker_pending_resubmit_transition_with(
        &state,
        &path_dialog,
        &path_generation,
        &refresh_dialog,
        &refresh_generation,
        &refresh_close_generation,
        &reopen,
        &open_slots,
        pending_a,
    ));
    assert_eq!(path_generation.load(Ordering::SeqCst), 9);
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 10);
    assert_eq!(reopen.load(Ordering::SeqCst), 1);
    assert_eq!(open_slots.load(Ordering::SeqCst), 0);
    assert!(claim_picker_pending_resubmit_transition_with(
        &state,
        &path_dialog,
        &path_generation,
        &refresh_dialog,
        &refresh_generation,
        &refresh_close_generation,
        &reopen,
        &open_slots,
        pending_b,
    ));
    assert_eq!(path_dialog.load(Ordering::SeqCst), 0);
    assert_eq!(path_generation.load(Ordering::SeqCst), 0);
    assert_eq!(refresh_dialog.load(Ordering::SeqCst), 0);
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 0);
    assert_eq!(reopen.load(Ordering::SeqCst), 0);
    assert_eq!(open_slots.load(Ordering::SeqCst), 0);
    assert!(state.lock().unwrap().pending.is_none());
}

#[test]
fn old_pending_rejects_newer_refresh_latch_before_new_pending_arm_then_exact_retry_clears_once() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let lineage =
        seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4100);
    let submits = AtomicUsize::new(0);
    let old = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 0,
        refresh_owner_generation: 8,
        refresh_close_generation: 0,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let newer = PickerPendingResubmitTransition {
        refresh_owner_generation: 9,
        resubmit_generation: 11,
        ..old
    };
    let state = std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 11,
        pending: Some(old),
        ..PickerPendingResubmitState::default()
    });
    let path_dialog = AtomicUsize::new(0);
    let path_generation = AtomicUsize::new(0);
    // Cross-thread order: the refresh pair publishes first; pending identity still names old.
    let refresh_dialog = AtomicUsize::new(0x5000);
    let refresh_generation = AtomicUsize::new(9);
    let refresh_close = AtomicUsize::new(0);
    let reopen = AtomicUsize::new(1);
    let open_slots = AtomicUsize::new(0);
    let old_authority = PickerOwnerClearedAuthority {
        observed_job: 0x4100,
        lineage,
        pending: old,
    };
    assert!(
        execute_owner_zero_submit_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    old_authority,
                    coordinator.cleared_lineage_for_job(0x4100),
                    state.lock().unwrap().pending,
                )
            },
            || {
                claim_picker_pending_resubmit_transition_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    old,
                )
            },
            || submits.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        )
        .is_none()
    );
    assert_eq!(state.lock().unwrap().pending, Some(old));
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 9);
    assert_eq!(reopen.load(Ordering::SeqCst), 1);
    assert_eq!(open_slots.load(Ordering::SeqCst), 0);

    assert_eq!(submits.load(Ordering::SeqCst), 0);
    state.lock().unwrap().pending = Some(newer);
    let exact_authority = PickerOwnerClearedAuthority {
        pending: newer,
        ..old_authority
    };
    assert!(
        execute_owner_zero_submit_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    exact_authority,
                    coordinator.cleared_lineage_for_job(0x4100),
                    state.lock().unwrap().pending,
                )
            },
            || {
                claim_picker_pending_resubmit_transition_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    newer,
                )
            },
            || submits.fetch_add(1, Ordering::SeqCst),
            |request| apply_test_owner_publication(&published, &applied, request),
        )
        .is_some()
    );
    assert_eq!(refresh_dialog.load(Ordering::SeqCst), 0);
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 0);
    assert_eq!(reopen.load(Ordering::SeqCst), 0);
    assert_eq!(open_slots.load(Ordering::SeqCst), 0);
    assert_eq!(submits.load(Ordering::SeqCst), 1);
}

#[test]
fn all_latch_claim_rejects_zero_vs_present_path_open_and_reopen_without_partial_clear() {
    let transition = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 0,
        refresh_owner_generation: 0,
        refresh_close_generation: 0,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 12,
    };
    let state = std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 12,
        pending: Some(transition),
        ..PickerPendingResubmitState::default()
    });
    let path_dialog = AtomicUsize::new(0x5000);
    let path_generation = AtomicUsize::new(7);
    let refresh_dialog = AtomicUsize::new(0);
    let refresh_generation = AtomicUsize::new(0);
    let refresh_close = AtomicUsize::new(0);
    let reopen = AtomicUsize::new(1);
    let open_slots = AtomicUsize::new(0);
    let claim = || {
        claim_picker_pending_resubmit_transition_with(
            &state,
            &path_dialog,
            &path_generation,
            &refresh_dialog,
            &refresh_generation,
            &refresh_close,
            &reopen,
            &open_slots,
            transition,
        )
    };
    assert!(!claim());
    assert_eq!(state.lock().unwrap().pending, Some(transition));
    path_dialog.store(0, Ordering::SeqCst);
    path_generation.store(0, Ordering::SeqCst);
    open_slots.store(1, Ordering::SeqCst);
    assert!(!claim());
    assert_eq!(reopen.load(Ordering::SeqCst), 1);
    open_slots.store(0, Ordering::SeqCst);
    reopen.store(0, Ordering::SeqCst);
    assert!(!claim());
    assert_eq!(state.lock().unwrap().pending, Some(transition));
    reopen.store(1, Ordering::SeqCst);
    assert!(claim());
    assert!(state.lock().unwrap().pending.is_none());
}

#[test]
fn owner_cleared_transaction_rejects_stale_newer_refresh_aba_and_wrong_job_before_stage() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let lineage =
        seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4100);
    let pending = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 0,
        refresh_owner_generation: 8,
        refresh_close_generation: 8,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let newer = PickerPendingResubmitTransition {
        refresh_owner_generation: 9,
        refresh_close_generation: 9,
        resubmit_generation: 11,
        ..pending
    };
    let state = std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 11,
        pending: Some(newer),
        ..PickerPendingResubmitState::default()
    });
    let path_dialog = AtomicUsize::new(0);
    let path_generation = AtomicUsize::new(0);
    let refresh_dialog = AtomicUsize::new(0x5000);
    let refresh_generation = AtomicUsize::new(9);
    let refresh_close = AtomicUsize::new(9);
    let reopen = AtomicUsize::new(1);
    let open_slots = AtomicUsize::new(0);
    let stages = AtomicUsize::new(0);
    let submits = AtomicUsize::new(0);
    let run = |authority: PickerOwnerClearedAuthority| {
        execute_owner_zero_resubmit_transaction_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    authority,
                    coordinator.cleared_lineage_for_job(authority.observed_job),
                    state.lock().unwrap().pending,
                )
            },
            || {
                reserve_picker_pending_resubmit_transition_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    authority.pending,
                )
            },
            || {
                stages.fetch_add(1, Ordering::SeqCst);
                true
            },
            || true,
            || {},
            |reservation| {
                commit_picker_pending_resubmit_reservation_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    reservation,
                )
            },
            |reservation| {
                let _ = release_picker_pending_resubmit_reservation_with(&state, reservation);
            },
            || {},
            || {},
            || {
                submits.fetch_add(1, Ordering::SeqCst);
                true
            },
            |request| apply_test_owner_publication(&published, &applied, request),
        )
    };
    for authority in [
        PickerOwnerClearedAuthority {
            observed_job: 0x4100,
            lineage,
            pending,
        },
        PickerOwnerClearedAuthority {
            observed_job: 0x4300,
            lineage,
            pending: newer,
        },
    ] {
        assert_eq!(run(authority), PickerResubmitDisposition::AuthorizationLost);
    }
    // Exact authority with a newer refresh pair than its bound transition also stops at reserve.
    state.lock().unwrap().pending = Some(pending);
    let exact_but_newer_refresh = PickerOwnerClearedAuthority {
        observed_job: 0x4100,
        lineage,
        pending,
    };
    assert_eq!(
        run(exact_but_newer_refresh),
        PickerResubmitDisposition::AuthorizationLost
    );
    let aba = seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4200);
    assert_ne!(lineage, aba);
    assert_eq!(
        run(exact_but_newer_refresh),
        PickerResubmitDisposition::AuthorizationLost
    );
    assert_eq!(stages.load(Ordering::SeqCst), 0);
    assert_eq!(submits.load(Ordering::SeqCst), 0);
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 9);
    assert_eq!(refresh_close.load(Ordering::SeqCst), 9);
    assert_eq!(reopen.load(Ordering::SeqCst), 1);
}

#[test]
fn authority_change_during_reserved_stage_rolls_back_then_exact_retry_stages_and_submits_once() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let lineage =
        seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4100);
    let pending_a = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 0,
        refresh_owner_generation: 8,
        refresh_close_generation: 8,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let pending_b = PickerPendingResubmitTransition {
        refresh_owner_generation: 9,
        refresh_close_generation: 9,
        resubmit_generation: 11,
        ..pending_a
    };
    let state = std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 10,
        pending: Some(pending_a),
        ..PickerPendingResubmitState::default()
    });
    let path_dialog = AtomicUsize::new(0);
    let path_generation = AtomicUsize::new(0);
    let refresh_dialog = AtomicUsize::new(0x5000);
    let refresh_generation = AtomicUsize::new(8);
    let refresh_close = AtomicUsize::new(8);
    let reopen = AtomicUsize::new(1);
    let open_slots = AtomicUsize::new(0);
    let stages = AtomicUsize::new(0);
    let rollbacks = AtomicUsize::new(0);
    let submits = AtomicUsize::new(0);
    let authority_a = PickerOwnerClearedAuthority {
        observed_job: 0x4100,
        lineage,
        pending: pending_a,
    };
    let validate_a = || {
        picker_owner_cleared_authority_matches(
            authority_a,
            coordinator.cleared_lineage_for_job(0x4100),
            state.lock().unwrap().pending,
        )
    };
    assert_eq!(
        execute_owner_zero_resubmit_transaction_on_coordinator_with(
            &coordinator,
            validate_a,
            || {
                reserve_picker_pending_resubmit_transition_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    pending_a,
                )
            },
            || {
                stages.fetch_add(1, Ordering::SeqCst);
                refresh_generation.store(9, Ordering::SeqCst);
                refresh_close.store(9, Ordering::SeqCst);
                state.lock().unwrap().pending = Some(pending_b);
                true
            },
            || true,
            || {},
            |reservation| {
                commit_picker_pending_resubmit_reservation_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    reservation,
                )
            },
            |reservation| {
                let _ = release_picker_pending_resubmit_reservation_with(&state, reservation);
            },
            || {
                rollbacks.fetch_add(1, Ordering::SeqCst);
            },
            || {},
            || {
                submits.fetch_add(1, Ordering::SeqCst);
                true
            },
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        PickerResubmitDisposition::AuthorizationLost
    );
    assert_eq!(stages.load(Ordering::SeqCst), 1);
    assert_eq!(rollbacks.load(Ordering::SeqCst), 1);
    assert_eq!(submits.load(Ordering::SeqCst), 0);
    assert_eq!(state.lock().unwrap().pending, Some(pending_b));
    assert!(state.lock().unwrap().reservation.is_none());
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 9);
    assert_eq!(reopen.load(Ordering::SeqCst), 1);

    let authority_b = PickerOwnerClearedAuthority {
        pending: pending_b,
        ..authority_a
    };
    assert_eq!(
        execute_owner_zero_resubmit_transaction_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    authority_b,
                    coordinator.cleared_lineage_for_job(0x4100),
                    state.lock().unwrap().pending,
                )
            },
            || {
                reserve_picker_pending_resubmit_transition_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    pending_b,
                )
            },
            || {
                stages.fetch_add(1, Ordering::SeqCst);
                true
            },
            || true,
            || {},
            |reservation| {
                commit_picker_pending_resubmit_reservation_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    reservation,
                )
            },
            |reservation| {
                let _ = release_picker_pending_resubmit_reservation_with(&state, reservation);
            },
            || {
                rollbacks.fetch_add(1, Ordering::SeqCst);
            },
            || {},
            || {
                assert_eq!(published.load(Ordering::SeqCst), 0);
                assert_eq!(
                    coordinator.publish_with(
                        PickerOwnerPublicationRequest::Set {
                            new_dialog: 0x6000,
                            job: 0x4200,
                        },
                        |request| apply_test_owner_publication(&published, &applied, request),
                    ),
                    PickerOwnerPublicationDisposition::Deferred
                );
                submits.fetch_add(1, Ordering::SeqCst);
                true
            },
            |request| apply_test_owner_publication(&published, &applied, request),
        ),
        PickerResubmitDisposition::Submitted { opened: true }
    );
    assert_eq!(stages.load(Ordering::SeqCst), 2);
    assert_eq!(rollbacks.load(Ordering::SeqCst), 1);
    assert_eq!(submits.load(Ordering::SeqCst), 1);
    assert_eq!(published.load(Ordering::SeqCst), 0x6000);
    assert!(state.lock().unwrap().pending.is_none());
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 0);
    assert_eq!(reopen.load(Ordering::SeqCst), 0);
}

#[test]
fn stage_failure_releases_reservation_without_latch_loss_then_exact_retry_succeeds() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let lineage =
        seed_test_owner_cleared_lineage(&coordinator, &published, &applied, 0x5000, 0x4100);
    let pending = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 7,
        refresh_owner_generation: 0,
        refresh_close_generation: 0,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let state = std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 10,
        pending: Some(pending),
        ..PickerPendingResubmitState::default()
    });
    let path_dialog = AtomicUsize::new(0x5000);
    let path_generation = AtomicUsize::new(7);
    let refresh_dialog = AtomicUsize::new(0);
    let refresh_generation = AtomicUsize::new(0);
    let refresh_close = AtomicUsize::new(0);
    let reopen = AtomicUsize::new(1);
    let open_slots = AtomicUsize::new(0);
    let stages = AtomicUsize::new(0);
    let rollbacks = AtomicUsize::new(0);
    let submits = AtomicUsize::new(0);
    let authority = PickerOwnerClearedAuthority {
        observed_job: 0x4100,
        lineage,
        pending,
    };
    let run = |stage_ok: bool| {
        execute_owner_zero_resubmit_transaction_on_coordinator_with(
            &coordinator,
            || {
                picker_owner_cleared_authority_matches(
                    authority,
                    coordinator.cleared_lineage_for_job(0x4100),
                    state.lock().unwrap().pending,
                )
            },
            || {
                reserve_picker_pending_resubmit_transition_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    pending,
                )
            },
            || {
                stages.fetch_add(1, Ordering::SeqCst);
                stage_ok
            },
            || true,
            || {},
            |reservation| {
                commit_picker_pending_resubmit_reservation_with(
                    &state,
                    &path_dialog,
                    &path_generation,
                    &refresh_dialog,
                    &refresh_generation,
                    &refresh_close,
                    &reopen,
                    &open_slots,
                    reservation,
                )
            },
            |reservation| {
                let _ = release_picker_pending_resubmit_reservation_with(&state, reservation);
            },
            || {
                rollbacks.fetch_add(1, Ordering::SeqCst);
            },
            || {},
            || {
                submits.fetch_add(1, Ordering::SeqCst);
                true
            },
            |request| apply_test_owner_publication(&published, &applied, request),
        )
    };
    assert_eq!(run(false), PickerResubmitDisposition::StageFailed);
    assert_eq!(state.lock().unwrap().pending, Some(pending));
    assert!(state.lock().unwrap().reservation.is_none());
    assert_eq!(path_generation.load(Ordering::SeqCst), 7);
    assert_eq!(reopen.load(Ordering::SeqCst), 1);
    assert_eq!(submits.load(Ordering::SeqCst), 0);
    assert_eq!(
        run(true),
        PickerResubmitDisposition::Submitted { opened: true }
    );
    assert_eq!(stages.load(Ordering::SeqCst), 2);
    assert_eq!(rollbacks.load(Ordering::SeqCst), 1);
    assert_eq!(submits.load(Ordering::SeqCst), 1);
    assert!(state.lock().unwrap().pending.is_none());
}

#[test]
fn system_dialog_loss_abandons_once_and_preserves_newer_unrelated_generation() {
    let exact = PickerPendingResubmitTransition {
        old_dialog: 0x5000,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: 7,
        refresh_owner_generation: 8,
        refresh_close_generation: 8,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation: 10,
    };
    let state = std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 10,
        pending: Some(exact),
        reservation: Some(PickerPendingResubmitReservation {
            transition: exact,
            reservation_generation: 3,
        }),
        ..PickerPendingResubmitState::default()
    });
    let path_dialog = AtomicUsize::new(0x5000);
    let path_generation = AtomicUsize::new(7);
    let refresh_dialog = AtomicUsize::new(0x5000);
    let refresh_generation = AtomicUsize::new(8);
    let refresh_close = AtomicUsize::new(8);
    let reopen = AtomicUsize::new(1);
    let open_slots = AtomicUsize::new(0);
    let abandon = |expected| {
        abandon_picker_pending_resubmit_with(
            &state,
            &path_dialog,
            &path_generation,
            &refresh_dialog,
            &refresh_generation,
            &refresh_close,
            &reopen,
            &open_slots,
            expected,
        )
    };
    assert_eq!(
        abandon_lost_system_dialog_resubmit_with(0, Some(exact), &abandon),
        Some(true)
    );
    assert_eq!(
        abandon_lost_system_dialog_resubmit_with(0, Some(exact), &abandon),
        Some(false)
    );
    assert_eq!(
        abandon_lost_system_dialog_resubmit_with(0x7000, Some(exact), &abandon),
        None
    );
    let rearms = AtomicUsize::new(0);
    let stages = AtomicUsize::new(0);
    let submits = AtomicUsize::new(0);
    for _ in 0..3 {
        if load_picker_refresh_request_with(&refresh_dialog, &refresh_generation).is_some() {
            rearms.fetch_add(1, Ordering::SeqCst);
        }
        if picker_resubmit_pending_with(
            reopen.load(Ordering::SeqCst),
            open_slots.load(Ordering::SeqCst),
        ) {
            stages.fetch_add(1, Ordering::SeqCst);
            submits.fetch_add(1, Ordering::SeqCst);
        }
    }
    assert_eq!(rearms.load(Ordering::SeqCst), 0);
    assert_eq!(stages.load(Ordering::SeqCst), 0);
    assert_eq!(submits.load(Ordering::SeqCst), 0);
    assert!(state.lock().unwrap().pending.is_none());
    assert!(state.lock().unwrap().reservation.is_none());
    assert!(load_picker_refresh_request_with(&refresh_dialog, &refresh_generation).is_none());
    assert!(!picker_resubmit_pending_with(
        reopen.load(Ordering::SeqCst),
        open_slots.load(Ordering::SeqCst)
    ));
    assert_eq!(path_generation.load(Ordering::SeqCst), 0);
    assert_eq!(refresh_close.load(Ordering::SeqCst), 0);

    let newer = PickerPendingResubmitTransition {
        old_dialog: 0x6000,
        path_owner_generation: 17,
        refresh_owner_generation: 18,
        refresh_close_generation: 18,
        resubmit_generation: 11,
        ..exact
    };
    {
        let mut guard = state.lock().unwrap();
        guard.pending = Some(newer);
        guard.reservation = Some(PickerPendingResubmitReservation {
            transition: exact,
            reservation_generation: 4,
        });
    }
    path_dialog.store(0x6000, Ordering::SeqCst);
    path_generation.store(17, Ordering::SeqCst);
    refresh_dialog.store(0x6000, Ordering::SeqCst);
    refresh_generation.store(18, Ordering::SeqCst);
    refresh_close.store(18, Ordering::SeqCst);
    reopen.store(1, Ordering::SeqCst);
    assert!(abandon(Some(exact)));
    let guard = state.lock().unwrap();
    assert_eq!(guard.pending, Some(newer));
    assert!(guard.reservation.is_none());
    drop(guard);
    assert_eq!(path_generation.load(Ordering::SeqCst), 17);
    assert_eq!(refresh_generation.load(Ordering::SeqCst), 18);
    assert_eq!(refresh_close.load(Ordering::SeqCst), 18);
    assert_eq!(reopen.load(Ordering::SeqCst), 1);
}

#[test]
fn refresh_waits_without_exact_token_and_live_token_closes_once() {
    let request = PickerRefreshRequest {
        dialog: 0x5000,
        generation: 7,
    };
    let closes = AtomicUsize::new(0);
    assert_eq!(
        consume_picker_refresh_with(
            request,
            0x5000,
            true,
            false,
            false,
            false,
            PickerProfileRunObservation::OtherResource,
            || panic!("generic post must not arm reopen"),
            |_| {
                closes.fetch_add(1, Ordering::SeqCst);
                PickerRefreshCloseDisposition::Closed
            },
        ),
        PickerRefreshConsumeDisposition::AwaitingLiveToken
    );
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    assert_eq!(
        consume_picker_refresh_with(
            request,
            0x5000,
            true,
            false,
            false,
            false,
            live_observation(0x5000),
            || {},
            |_| {
                closes.fetch_add(1, Ordering::SeqCst);
                PickerRefreshCloseDisposition::Closed
            },
        ),
        PickerRefreshConsumeDisposition::CloseRequested(PickerRefreshCloseDisposition::Closed)
    );
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

#[test]
fn matching_path_return_forbids_refresh_close_until_owner_zero_then_retires() {
    let state = TestRefreshState::new();
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("changed content generation must queue, got {other:?}"),
    };
    state.reopen.store(1, Ordering::SeqCst);
    let closes = AtomicUsize::new(0);
    assert_eq!(
        consume_picker_refresh_with(
            request,
            request.dialog,
            true,
            false,
            false,
            true,
            live_observation(request.dialog),
            || panic!("live matching return must wait, not re-arm"),
            |_| {
                closes.fetch_add(1, Ordering::SeqCst);
                PickerRefreshCloseDisposition::Closed
            },
        ),
        PickerRefreshConsumeDisposition::AwaitingOwnerDisappearance
    );
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    let stages = AtomicUsize::new(0);
    assert_eq!(
        consume_picker_refresh_with(
            request,
            0,
            true,
            false,
            false,
            true,
            PickerProfileRunObservation::OtherResource,
            || {
                stages.fetch_add(1, Ordering::SeqCst);
            },
            |_| panic!("owner zero must never call close sink"),
        ),
        PickerRefreshConsumeDisposition::OwnerAlreadyCleared
    );
    assert_eq!(stages.load(Ordering::SeqCst), 1);
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    assert!(state.retire(request, true));
    state.assert_cleared(1);
    assert_eq!(closes.load(Ordering::SeqCst), 0);
}

#[test]
fn refresh_owner_zero_arms_reopen_without_close_and_different_owner_is_stale() {
    let request = PickerRefreshRequest {
        dialog: 0x5000,
        generation: 8,
    };
    let arms = AtomicUsize::new(0);
    assert_eq!(
        consume_picker_refresh_with(
            request,
            0,
            true,
            false,
            false,
            true,
            PickerProfileRunObservation::OtherResource,
            || {
                arms.fetch_add(1, Ordering::SeqCst);
            },
            |_| panic!("owner-zero must never close"),
        ),
        PickerRefreshConsumeDisposition::OwnerAlreadyCleared
    );
    assert_eq!(arms.load(Ordering::SeqCst), 1);
    assert_eq!(
        consume_picker_refresh_with(
            request,
            0x6000,
            true,
            false,
            false,
            false,
            live_observation(0x6000),
            || panic!("stale owner must not arm"),
            |_| panic!("stale owner must not close"),
        ),
        PickerRefreshConsumeDisposition::StaleIdentity
    );
}

#[test]
fn path_editor_return_reopen_is_exact_generation_and_never_calls_close() {
    let pending_dialog = AtomicUsize::new(0);
    let pending_generation = AtomicUsize::new(0);
    let reopen = AtomicUsize::new(0);
    let request = PathEditorReturnReopenRequest {
        dialog: 0x5000,
        generation: 9,
    };
    assert_eq!(
        queue_path_editor_return_reopen_with(
            &pending_dialog,
            &pending_generation,
            &reopen,
            request,
        ),
        PathEditorReturnReopenDisposition::Queued(request)
    );
    assert_eq!(reopen.load(Ordering::SeqCst), 1);
    assert_eq!(
        queue_path_editor_return_reopen_with(
            &pending_dialog,
            &pending_generation,
            &reopen,
            request,
        ),
        PathEditorReturnReopenDisposition::Coalesced(request)
    );
    assert_eq!(
        queue_path_editor_return_reopen_with(
            &pending_dialog,
            &pending_generation,
            &reopen,
            PathEditorReturnReopenRequest {
                generation: 10,
                ..request
            },
        ),
        PathEditorReturnReopenDisposition::Rejected
    );
}

#[test]
fn owning_list_absence_requires_every_slot_to_be_readable() {
    assert_eq!(
        picker_owner_list_presence_with(0x5000, Some(2), |index| {
            [Some(0x4000), Some(0x5000)][index]
        }),
        PickerOwnerListPresence::Present
    );
    assert_eq!(
        picker_owner_list_presence_with(0x5000, Some(2), |index| {
            [Some(0x4000), Some(0x6000)][index]
        }),
        PickerOwnerListPresence::Absent
    );
    assert_eq!(
        picker_owner_list_presence_with(0x5000, Some(2), |index| { [Some(0x4000), None][index] }),
        PickerOwnerListPresence::Ambiguous
    );
    assert_eq!(
        picker_owner_list_presence_with(0x5000, Some(9), |_| Some(0)),
        PickerOwnerListPresence::Ambiguous
    );
}

#[test]
fn exact_absent_owner_publishes_zero_once_and_ambiguous_state_waits() {
    let publishes = AtomicUsize::new(0);
    assert_eq!(
        publish_absent_picker_owner_with(0x5000, 0x5000, true, || {
            publishes.fetch_add(1, Ordering::SeqCst);
            0x5000
        }),
        PickerAbsentOwnerPublication::Published
    );
    assert_eq!(publishes.load(Ordering::SeqCst), 1);
    assert_eq!(
        publish_absent_picker_owner_with(0x5000, 0x5000, false, || {
            panic!("ambiguous absence must not publish")
        }),
        PickerAbsentOwnerPublication::Ambiguous
    );
    assert_eq!(
        publish_absent_picker_owner_with(0x5000, 0x6000, true, || {
            panic!("stale owner must not publish")
        }),
        PickerAbsentOwnerPublication::Stale
    );
}

#[test]
fn last_moment_close_preflight_rejection_retains_exact_refresh_for_owner_evidence() {
    let state = TestRefreshState::new();
    let request = match state.queue(0x5000) {
        PickerRefreshRequestDisposition::Queued(request) => request,
        other => panic!("first request must queue, got {other:?}"),
    };
    state.reopen.store(1, Ordering::SeqCst);
    assert_eq!(
        apply_picker_refresh_close_with(
            request,
            PickerRefreshCloseDisposition::PreflightRejected,
            |request, keep| state.retire(request, keep),
        ),
        PickerRefreshCloseResolution::Retained
    );
    state.assert_request(request);
    assert_eq!(state.reopen.load(Ordering::SeqCst), 1);
}

#[test]
fn deferred_native_close_retry_requires_the_exact_live_profile_token() {
    assert!(!picker_deferred_close_token_allows(
        PickerProfileRunObservation::OtherResource,
        0x5000
    ));
    assert!(!picker_deferred_close_token_allows(
        live_observation(0x6000),
        0x5000
    ));
    assert!(picker_deferred_close_token_allows(
        live_observation(0x5000),
        0x5000
    ));
}

#[test]
fn same_address_new_generation_retires_old_path_editor_return() {
    let old = PathEditorReturnReopenRequest {
        dialog: 0x5000,
        generation: 7,
    };
    assert!(path_editor_return_matches_owner(old, 0x5000, 7));
    assert!(!path_editor_return_matches_owner(old, 0x5000, 8));
    assert!(!path_editor_return_matches_owner(old, 0x6000, 7));
}
