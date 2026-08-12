fn native_removal_pending(
    token: PickerProfileRunToken,
    resubmit_generation: usize,
) -> PickerPendingResubmitTransition {
    PickerPendingResubmitTransition {
        old_dialog: token.dialog,
        system_dialog: 0x7000,
        system_dialog_generation: 3,
        path_owner_generation: token.owner_generation,
        refresh_owner_generation: 0,
        refresh_close_generation: 0,
        reopen_pending: 1,
        open_slots_pending: 0,
        resubmit_generation,
    }
}

#[test]
fn escape_native_removal_ticket_gives_the_next_generic_post_one_stage_and_submit() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let dialog = 0x18df_c4080;
    let job = 0x186f_10f80;
    let token = seed_test_live_token(&coordinator, &published, &applied, dialog, job, 0x142b229f8);
    let capture = coordinator
        .capture_native_removal(dialog, job, token.list)
        .expect("exact 05_010 Run/list capture");
    let pending = native_removal_pending(token, 11);
    let original_calls = AtomicUsize::new(0);
    let list_contains = std::cell::Cell::new(true);
    let job_owner = std::cell::Cell::new(dialog);
    let profile_posts_after_remove = AtomicUsize::new(0);

    // Pre-fix state: a generic post is never arbitrary picker-submit authority.
    assert!(
        !picker_outer_post_permissions_with(PickerOuterPostAuthority::Other, false, false, true)
            .picker_submit
    );
    let disposition = native_menu_window_removal_boundary_with(
        &coordinator,
        Some(capture),
        || {
            original_calls.fetch_add(1, Ordering::SeqCst);
            list_contains.set(false);
            job_owner.set(0);
        },
        |_| !list_contains.get() && job_owner.get() == 0,
        |capture| {
            picker_pending_resubmit_matches_native_removal_with(
                Some(pending),
                false,
                true,
                Some(PickerSystemDialogIdentity {
                    dialog: 0x7000,
                    generation: 3,
                }),
                true,
                capture,
            )
            .then_some(pending)
        },
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(disposition, PickerNativeRemovalDisposition::Published);
    assert_eq!(original_calls.load(Ordering::SeqCst), 1);
    assert_eq!(published.load(Ordering::SeqCst), 0);
    assert_eq!(profile_posts_after_remove.load(Ordering::SeqCst), 0);

    let authority = coordinator
        .native_removal_authority()
        .expect("native removal creates one-shot owner-cleared ticket");
    let generic = PickerOuterPostAuthority::NativeRemoval(authority);
    assert!(picker_outer_post_permissions_with(generic, false, false, true).picker_submit);
    assert!(picker_outer_authority_still_current_with(
        generic,
        || 0,
        || 0,
        |_| false,
        |candidate| candidate == authority,
        |_| panic!("native-removal authority must not dereference the removed owner"),
    ));
    let stages = AtomicUsize::new(0);
    let submits = AtomicUsize::new(0);
    let disposition = execute_owner_zero_resubmit_transaction_on_coordinator_with(
        &coordinator,
        || coordinator.native_removal_authority_is_current(authority),
        || Some(1_u8),
        || {
            stages.fetch_add(1, Ordering::SeqCst);
            true
        },
        || true,
        || {},
        |_| {},
        |_| {},
        || {},
        || {},
        || {
            submits.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                coordinator.publish_with(
                    PickerOwnerPublicationRequest::Set {
                        new_dialog: 0x8000,
                        job: job + 1,
                    },
                    |request| apply_test_owner_publication(&published, &applied, request),
                ),
                PickerOwnerPublicationDisposition::Deferred
            );
            true
        },
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(
        disposition,
        PickerResubmitDisposition::Submitted { opened: true }
    );
    assert_eq!(published.load(Ordering::SeqCst), 0x8000);
    assert_eq!(
        coordinator.snapshot_for_test().2.map(|owner| owner.dialog),
        Some(0x8000)
    );
    assert!(coordinator.commit_native_removal_authority(authority));
    assert_eq!(stages.load(Ordering::SeqCst), 1);
    assert_eq!(submits.load(Ordering::SeqCst), 1);
    assert_eq!(profile_posts_after_remove.load(Ordering::SeqCst), 0);
}

#[test]
fn native_removal_defers_under_close_lease_then_creates_the_exact_ticket_on_release() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let token = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x6000,
        0x142b229f8,
    );
    let capture = coordinator
        .capture_native_removal(token.dialog, token.job, token.list)
        .unwrap();
    let pending = native_removal_pending(token, 21);
    coordinator.begin_lease();
    let disposition = native_menu_window_removal_boundary_with(
        &coordinator,
        Some(capture),
        || {},
        |_| true,
        |_| Some(pending),
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(disposition, PickerNativeRemovalDisposition::Deferred);
    assert_eq!(published.load(Ordering::SeqCst), token.dialog);
    assert!(coordinator.native_removal_authority().is_none());
    coordinator
        .release_lease_with(|request| apply_test_owner_publication(&published, &applied, request));
    assert_eq!(published.load(Ordering::SeqCst), 0);
    let authority = coordinator
        .native_removal_authority()
        .expect("deferred removal publishes ticket at outer lease release");
    assert_eq!(authority.pending, pending);
    assert_eq!(authority.list, token.list);
    assert!(coordinator.native_removal_authority_is_current(authority));
}

#[test]
fn removal_ticket_retains_hidden_model_on_stage_and_native_false_then_commits_once() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let token = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x6000,
        0x142b229f8,
    );
    let capture = coordinator
        .capture_native_removal(token.dialog, token.job, token.list)
        .unwrap();
    let pending = native_removal_pending(token, 12);
    let disposition = native_menu_window_removal_boundary_with(
        &coordinator,
        Some(capture),
        || {},
        |_| true,
        |_| Some(pending),
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(disposition, PickerNativeRemovalDisposition::Published);
    let authority = coordinator.native_removal_authority().unwrap();
    let clears = AtomicUsize::new(0);
    let run = |stage_ok: bool, submit_ok: bool| {
        execute_owner_zero_resubmit_transaction_on_coordinator_with(
            &coordinator,
            || coordinator.native_removal_authority_is_current(authority),
            || Some(1_u8),
            || stage_ok,
            || true,
            || {},
            |_| {},
            |_| {},
            || {},
            || {},
            || submit_ok,
            |request| apply_test_owner_publication(&published, &applied, request),
        )
    };
    for (stage_ok, submit_ok, expected) in [
        (false, true, PickerResubmitDisposition::StageFailed),
        (
            true,
            false,
            PickerResubmitDisposition::Submitted { opened: false },
        ),
    ] {
        let result = run(stage_ok, submit_ok);
        assert_eq!(result, expected);
        apply_picker_resubmit_model_lifetime_with(result, true, || {
            clears.fetch_add(1, Ordering::SeqCst);
        });
        assert!(coordinator.native_removal_authority_is_current(authority));
        assert_eq!(clears.load(Ordering::SeqCst), 0);
    }
    let success = run(true, true);
    assert_eq!(
        success,
        PickerResubmitDisposition::Submitted { opened: true }
    );
    assert!(coordinator.commit_native_removal_authority(authority));
    apply_picker_resubmit_model_lifetime_with(success, true, || {
        clears.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(clears.load(Ordering::SeqCst), 1);
    assert!(!coordinator.commit_native_removal_authority(authority));
}

#[test]
fn removal_boundary_rejects_wrong_list_missing_transition_system_loss_aba_and_duplicate() {
    let coordinator = PickerOwnerLifetimeCoordinator::default();
    let published = AtomicUsize::new(0);
    let applied = AtomicUsize::new(0);
    let old = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        0x5000,
        0x6000,
        0x142b229f8,
    );
    let capture = coordinator
        .capture_native_removal(old.dialog, old.job, old.list)
        .unwrap();
    assert!(
        coordinator
            .capture_native_removal(old.dialog, old.job, old.list + 8)
            .is_none()
    );
    assert!(
        coordinator
            .capture_native_removal(old.dialog, old.job + 1, old.list)
            .is_none()
    );

    let no_transition = native_menu_window_removal_boundary_with(
        &coordinator,
        Some(capture),
        || {},
        |_| true,
        |_| None,
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(no_transition, PickerNativeRemovalDisposition::NoTransition);
    let not_removed = native_menu_window_removal_boundary_with(
        &coordinator,
        Some(capture),
        || {},
        |_| false,
        |_| Some(native_removal_pending(old, 1)),
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(
        not_removed,
        PickerNativeRemovalDisposition::RemovalNotProven
    );

    // Same address, newer job/owner/Run lineage invalidates the old capture (pointer ABA).
    let newer = seed_test_live_token(
        &coordinator,
        &published,
        &applied,
        old.dialog,
        old.job + 1,
        0x142b229f8,
    );
    let stale = native_menu_window_removal_boundary_with(
        &coordinator,
        Some(capture),
        || {},
        |_| true,
        |_| Some(native_removal_pending(old, 2)),
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(stale, PickerNativeRemovalDisposition::Stale);

    let current = coordinator
        .capture_native_removal(newer.dialog, newer.job, newer.list)
        .unwrap();
    let pending = native_removal_pending(newer, 3);
    // Lost System identity makes the production transition predicate reject before publication.
    assert!(!picker_pending_resubmit_matches_native_removal_with(
        Some(pending),
        false,
        true,
        None,
        true,
        current,
    ));
    let published_once = native_menu_window_removal_boundary_with(
        &coordinator,
        Some(current),
        || {},
        |_| true,
        |_| Some(pending),
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(published_once, PickerNativeRemovalDisposition::Published);
    let duplicate = native_menu_window_removal_boundary_with(
        &coordinator,
        coordinator.capture_native_removal(newer.dialog, newer.job, newer.list),
        || {},
        |_| true,
        |_| Some(pending),
        |request| apply_test_owner_publication(&published, &applied, request),
    );
    assert_eq!(duplicate, PickerNativeRemovalDisposition::Foreign);
}

#[test]
fn fixed_vector_membership_and_refresh_handoff_are_exact() {
    let vector = 0x1000;
    let slots = [(vector, 0x5000), (vector + 8, 0x6000)];
    assert_eq!(
        menu_window_push_target_contains_with(vector, 0x5000, 2, |slot| {
            slots
                .iter()
                .find_map(|(address, value)| (*address == slot).then_some(*value))
        }),
        Some(true)
    );
    assert_eq!(
        menu_window_push_target_contains_with(vector, 0x7000, 2, |slot| {
            slots
                .iter()
                .find_map(|(address, value)| (*address == slot).then_some(*value))
        }),
        Some(false)
    );
    assert_eq!(
        menu_window_push_target_contains_with(vector, 0x5000, 9, |_| Some(0)),
        None
    );
    assert_eq!(
        menu_window_push_target_contains_with(vector, 0x5000, 1, |_| None),
        None
    );

    let token = live_token(0x5000);
    let pending = PickerPendingResubmitTransition {
        refresh_owner_generation: 9,
        ..native_removal_pending(token, 1)
    };
    let authority = PickerNativeRemovalAuthority {
        pending,
        cleared: PickerOwnerClearedLineage {
            old_owner: PickerOwnerLineage {
                dialog: token.dialog,
                generation: token.owner_generation,
                job: token.job,
                job_lineage: token.job_lineage,
            },
            old_run: live_run_registration(token.job),
            zero_generation: 4,
        },
        list: token.list,
    };
    assert!(picker_native_removal_matches_refresh(
        authority,
        PickerRefreshRequest {
            dialog: token.dialog,
            generation: 9,
        }
    ));
    assert!(!picker_native_removal_matches_refresh(
        authority,
        PickerRefreshRequest {
            dialog: token.dialog,
            generation: 10,
        }
    ));

    let owner_zero_loop_entries = AtomicUsize::new(0);
    for _ in 0..128 {
        let disposition = consume_picker_refresh_with_native_removal(
            true,
            PickerRefreshRequest {
                dialog: token.dialog,
                generation: 9,
            },
            0,
            true,
            false,
            false,
            false,
            PickerProfileRunObservation::OtherResource,
            || {
                owner_zero_loop_entries.fetch_add(1, Ordering::SeqCst);
            },
            |_| panic!("ticket handoff cannot close a generic resource"),
        );
        assert_eq!(
            disposition,
            PickerRefreshConsumeDisposition::NativeRemovalHandoff
        );
    }
    assert_eq!(owner_zero_loop_entries.load(Ordering::SeqCst), 0);
}
