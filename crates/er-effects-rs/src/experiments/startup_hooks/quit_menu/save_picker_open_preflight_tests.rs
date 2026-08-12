use super::*;

fn active_facts(owner: usize) -> PickerLoadSourceOpenFacts {
    PickerLoadSourceOpenFacts {
        mode_active: true,
        profile_owner: owner,
        profile_vtable: 0x142b229f8,
        expected_profile_vtable: 0x142b229f8,
        live_owner_authorized: true,
        activation_system: 0x7000,
        activation_action: 0x7170,
        tracked_system: 0x7000,
        tracked_action: 0x7170,
        owner_zero_resubmit_pending: false,
        exact_parent_authority: false,
    }
}

fn apply_open_effects_only_for_initial<G>(
    preflight: PickerLoadSourceOpenPreflightWith<G>,
    restores: &AtomicUsize,
    clears: &AtomicUsize,
    stages: &AtomicUsize,
    submits: &AtomicUsize,
) -> PickerLoadSourceOpenDecision {
    match preflight {
        PickerLoadSourceOpenPreflightWith::Initial(_) => {
            restores.fetch_add(1, Ordering::SeqCst);
            clears.fetch_add(1, Ordering::SeqCst);
            stages.fetch_add(1, Ordering::SeqCst);
            submits.fetch_add(1, Ordering::SeqCst);
            PickerLoadSourceOpenDecision::Initial
        }
        PickerLoadSourceOpenPreflightWith::Coalesced(decision, _) => decision,
        PickerLoadSourceOpenPreflightWith::Rejected(_) => PickerLoadSourceOpenDecision::Rejected,
    }
}

/// Regression for the wedged run of 2026-08-11 (artifacts:
/// `target/pi-local/claude-hover-fix/wedged-run-20260811T163400Z`). A path-editor cancel left the
/// owner published at a FREED ProfileSelect window; every subsequent `Load Character from File`
/// click was rejected, so the quit menu behaved as if it had no functionality at all. The values
/// below are the exact ones the run recorded.
#[test]
fn a_freed_profile_select_owner_is_released_and_stops_rejecting_every_future_open() {
    const WEDGED_OWNER: usize = 0x18beca080;
    const REUSED_HEAP_VTABLE: usize = 0x18beaa080;
    const PROFILE_LOAD_DIALOG_VTABLE: usize = 0x142b229f8;

    let dangling = PickerStaleOwnerFacts {
        profile_owner: WEDGED_OWNER,
        profile_vtable: REUSED_HEAP_VTABLE,
        expected_profile_vtable: PROFILE_LOAD_DIALOG_VTABLE,
        live_owner_authorized: false,
        owner_zero_resubmit_pending: false,
    };
    assert!(
        picker_owner_is_dangling(dangling),
        "a published owner whose vtable slot holds a heap pointer is freed memory"
    );

    // The Escape teardown (minimal repro: open Load Character from File, Escape, reopen) leaves the
    // freed page reading ZERO rather than a reused heap pointer. A live ProfileLoadDialog can never
    // have a null vtable, so zero is the strongest death evidence there is -- treating it as merely
    // "unreadable, leave it alone" is what made the first version of this invariant never fire.
    assert!(
        picker_owner_is_dangling(PickerStaleOwnerFacts {
            profile_owner: 0x179f46080,
            profile_vtable: 0,
            ..dangling
        }),
        "a null vtable on a published owner is a dead window, not an excuse to keep the pointer"
    );

    // The leak is what made the menu inert: with the owner non-zero and mode already cleared,
    // neither Coalesced branch can fire and Initial is impossible, so EVERY open is Rejected.
    let wedged_open = PickerLoadSourceOpenFacts {
        mode_active: false,
        profile_owner: WEDGED_OWNER,
        profile_vtable: REUSED_HEAP_VTABLE,
        expected_profile_vtable: PROFILE_LOAD_DIALOG_VTABLE,
        live_owner_authorized: false,
        activation_system: 0x18bec6080,
        activation_action: 0x189e8d8f0,
        tracked_system: 0,
        tracked_action: 0,
        owner_zero_resubmit_pending: false,
        exact_parent_authority: true,
    };
    assert_eq!(
        classify_picker_load_source_open(wedged_open),
        PickerLoadSourceOpenDecision::Rejected
    );
    // Releasing the dangling pointer is the whole repair: same facts, owner zeroed -> Initial.
    assert_eq!(
        classify_picker_load_source_open(PickerLoadSourceOpenFacts {
            profile_owner: 0,
            profile_vtable: 0,
            ..wedged_open
        }),
        PickerLoadSourceOpenDecision::Initial
    );

    // Fail closed: never steal a pointer a real transition still owns.
    for (label, facts) in [
        (
            "live authorized owner",
            PickerStaleOwnerFacts {
                live_owner_authorized: true,
                ..dangling
            },
        ),
        (
            "owner-zero transition in flight",
            PickerStaleOwnerFacts {
                owner_zero_resubmit_pending: true,
                ..dangling
            },
        ),
        (
            "healthy ProfileLoadDialog vtable",
            PickerStaleOwnerFacts {
                profile_vtable: PROFILE_LOAD_DIALOG_VTABLE,
                ..dangling
            },
        ),
        (
            "no expected vtable resolved",
            PickerStaleOwnerFacts {
                expected_profile_vtable: 0,
                ..dangling
            },
        ),
        (
            "no owner published",
            PickerStaleOwnerFacts {
                profile_owner: 0,
                ..dangling
            },
        ),
    ] {
        assert!(
            !picker_owner_is_dangling(facts),
            "{label} must not be treated as dangling"
        );
    }
}

/// Calling the signature must not deadlock. Trivial, and it is exactly what was missing on
/// 2026-08-11: the first version built the array inline, so the `pending_resubmit_state()` guard for
/// element 0 was still alive when `any_resubmit_reserved()` re-locked that same non-reentrant mutex
/// for element 5. One call would have hung; instead it shipped and soft-locked the game with the
/// menu pump thread wedged and the debug log frozen mid-line. A hung test here is the point -- the
/// suite failing by timeout is the detection.
#[test]
fn taking_the_transition_signature_never_re_locks_its_own_mutexes() {
    let first = save_picker_transition_signature();
    let second = save_picker_transition_signature();
    assert_eq!(
        first, second,
        "an idle transition signature must be stable across calls"
    );
    // The inert gate and the stall check read overlapping state; taking them back to back must not
    // nest their guards either.
    let _ = save_picker_system_rows_transition_owned();
    let _ = save_picker_system_rows_input_inert();
    let _ = save_picker_transition_signature();
}

/// All three observed leak shapes, from the three teardown routes that each leaked a different piece
/// of picker liveness on 2026-08-11. Judging them together is the invariant; fixing them one route
/// at a time is what let the wedge return three times.
#[test]
fn every_observed_orphaned_picker_shape_is_released_and_live_states_are_not() {
    let live = PickerOrphanFacts {
        mode_active: true,
        profile_owner: 0x18beca080,
        owner_dangling: false,
        tracked_system: 0x18bec6080,
        tracked_action: 0x189e8d8f0,
        real_windows_hidden: false,
        transition_owned: false,
    };
    assert!(
        !picker_state_is_orphaned(live),
        "a live picker with a healthy owner must never be reset"
    );

    // 1. Escape out of the picker: mode latched over a freed window (vtable read 0).
    assert!(picker_state_is_orphaned(PickerOrphanFacts {
        mode_active: true,
        profile_owner: 0x179f46080,
        owner_dangling: true,
        ..live
    }));
    // 2. Path-editor accept unwind: mode already clear, owner leaked at a reused allocation.
    assert!(picker_state_is_orphaned(PickerOrphanFacts {
        mode_active: false,
        profile_owner: 0x18beca080,
        owner_dangling: true,
        ..live
    }));
    // 3. Escape out of the software keyboard: mode AND owner both correctly cleared, but the
    //    tracked identity survived -- and `Initial` requires both of those to be zero, so this
    //    shape rejects every future open while looking perfectly healthy to an owner-only check.
    assert!(picker_state_is_orphaned(PickerOrphanFacts {
        mode_active: false,
        profile_owner: 0,
        owner_dangling: false,
        tracked_system: 0x18e6a0080,
        tracked_action: 0x18dfda2f0,
        real_windows_hidden: false,
        transition_owned: false,
    }));
    // 4. Clean back out of the picker: every picker field is correctly clear, but the real System
    //    windows we hid for the picker are still hidden, so a back press reveals the sibling native
    //    ProfileSelect (vanilla per-character Load Game) instead of the quit menu. Measured
    //    2026-08-11: 7 hides against 1 restore, because the only un-hide runs inside a 05_010 post
    //    and the picker stops posting the moment it closes.
    assert!(picker_state_is_orphaned(PickerOrphanFacts {
        mode_active: false,
        profile_owner: 0,
        owner_dangling: false,
        tracked_system: 0,
        tracked_action: 0,
        real_windows_hidden: true,
        transition_owned: false,
    }));

    // The 2026-08-11 ping-pong: a LIVE picker (healthy owner) whose System windows are hidden is
    // the correct state, not a wedge. Treating it as one released and re-hid 2,240 times.
    assert!(
        !picker_state_is_orphaned(PickerOrphanFacts {
            mode_active: false,
            profile_owner: 0x1900ba080,
            owner_dangling: false,
            tracked_system: 0,
            tracked_action: 0,
            real_windows_hidden: true,
            transition_owned: false,
        }),
        "hidden System windows behind a live picker window are correct, not orphaned"
    );

    // Fully clean state is not a wedge.
    assert!(!picker_state_is_orphaned(PickerOrphanFacts {
        mode_active: false,
        profile_owner: 0,
        owner_dangling: false,
        tracked_system: 0,
        tracked_action: 0,
        real_windows_hidden: false,
        transition_owned: false,
    }));
    // An in-flight transition owns every one of these shapes; never steal it mid-flight.
    for shape in [
        PickerOrphanFacts {
            mode_active: true,
            owner_dangling: true,
            transition_owned: true,
            ..live
        },
        PickerOrphanFacts {
            mode_active: false,
            profile_owner: 0,
            owner_dangling: false,
            transition_owned: true,
            ..live
        },
    ] {
        assert!(
            !picker_state_is_orphaned(shape),
            "an owned transition must keep the picker state"
        );
    }
}

#[test]
fn runtime_hover_resubmit_owner_b_then_underlying_load_activation_is_zero_mutation() {
    let restores = AtomicUsize::new(0);
    let clears = AtomicUsize::new(0);
    let stages = AtomicUsize::new(0);
    let submits = AtomicUsize::new(0);
    let model_generation = AtomicUsize::new(41);
    let presentation_generation = AtomicUsize::new(41);
    let owner_b = 0x85bb2080;

    // Runtime sequence: owner A was closed by hover, owner-zero staging/resubmit completed, and the
    // fresh exact owner B now runs. An exact duplicate coalesces without touching B.
    let exact_b = active_facts(owner_b);
    let exact = picker_load_source_open_preflight_with(
        exact_b,
        || -> Option<()> {
            panic!("a live-owner duplicate must never claim initial-open authority")
        },
        || panic!("a classified live-owner duplicate must not need retry facts"),
    );
    assert_eq!(
        apply_open_effects_only_for_initial(exact, &restores, &clears, &stages, &submits),
        PickerLoadSourceOpenDecision::CoalescedLive
    );

    // The rebuilt hidden System dialog/action is foreign to the picker-owned parent identity. It
    // fails closed before the open effects, leaving owner/model/presentation B unchanged.
    let foreign_underlying = PickerLoadSourceOpenFacts {
        activation_system: 0x85bb6080,
        activation_action: 0x3275e1f0,
        exact_parent_authority: true,
        ..exact_b
    };
    let rejected = picker_load_source_open_preflight_with(
        foreign_underlying,
        || -> Option<()> { panic!("foreign active ownership must not claim") },
        || panic!("foreign active ownership is terminally rejected"),
    );
    assert_eq!(
        apply_open_effects_only_for_initial(rejected, &restores, &clears, &stages, &submits),
        PickerLoadSourceOpenDecision::Rejected
    );
    assert_eq!(restores.load(Ordering::SeqCst), 0);
    assert_eq!(clears.load(Ordering::SeqCst), 0);
    assert_eq!(stages.load(Ordering::SeqCst), 0);
    assert_eq!(submits.load(Ordering::SeqCst), 0);
    assert_eq!(model_generation.load(Ordering::SeqCst), 41);
    assert_eq!(presentation_generation.load(Ordering::SeqCst), 41);
    assert_eq!(exact_b.profile_owner, owner_b);
}

#[test]
fn owner_zero_pending_duplicate_coalesces_without_open_effects() {
    let facts = PickerLoadSourceOpenFacts {
        profile_owner: 0,
        profile_vtable: 0,
        owner_zero_resubmit_pending: true,
        ..active_facts(0x5000)
    };
    let effects = AtomicUsize::new(0);
    let preflight = picker_load_source_open_preflight_with(
        facts,
        || -> Option<()> { panic!("owner-zero resubmit already owns the transition") },
        || panic!("owner-zero duplicate is classified immediately"),
    );
    match preflight {
        PickerLoadSourceOpenPreflightWith::Coalesced(
            PickerLoadSourceOpenDecision::CoalescedOwnerZero,
            observed,
        ) => assert_eq!(observed, facts),
        _ => panic!("exact owner-zero pending duplicate must coalesce"),
    }
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[test]
fn foreign_system_action_owner_or_vtable_reject_before_open_effects() {
    let exact = active_facts(0x5000);
    let mismatches = [
        PickerLoadSourceOpenFacts {
            activation_system: 0x8000,
            ..exact
        },
        PickerLoadSourceOpenFacts {
            activation_action: 0x8170,
            ..exact
        },
        PickerLoadSourceOpenFacts {
            profile_owner: 0x6000,
            profile_vtable: 0x142b229f7,
            ..exact
        },
        PickerLoadSourceOpenFacts {
            profile_owner: 0x6000,
            live_owner_authorized: false,
            ..exact
        },
        PickerLoadSourceOpenFacts {
            tracked_system: 0,
            tracked_action: 0,
            ..exact
        },
    ];
    for facts in mismatches {
        assert!(matches!(
            picker_load_source_open_preflight_with(
                facts,
                || -> Option<()> {
                    panic!("active mismatch must never claim initial-open authority")
                },
                || panic!("active mismatch rejects without retry"),
            ),
            PickerLoadSourceOpenPreflightWith::Rejected(_)
        ));
    }
}

#[test]
fn initial_open_requires_inactive_zero_owner_exact_parent_and_exclusive_boundary() {
    let initial = PickerLoadSourceOpenFacts {
        mode_active: false,
        profile_owner: 0,
        profile_vtable: 0,
        expected_profile_vtable: 0x142b229f8,
        live_owner_authorized: false,
        activation_system: 0x7000,
        activation_action: 0x7170,
        tracked_system: 0,
        tracked_action: 0,
        owner_zero_resubmit_pending: false,
        exact_parent_authority: true,
    };
    assert_eq!(
        classify_picker_load_source_open(initial),
        PickerLoadSourceOpenDecision::Initial
    );

    let coordinator = PickerResetTransactionCoordinator::default();
    let serialization = std::sync::Mutex::new(());
    let preflight = picker_load_source_open_preflight_with(
        initial,
        || coordinator.try_begin_exclusive_with(&serialization, || false, || true),
        || panic!("successful boundary claim needs no refresh"),
    );
    let PickerLoadSourceOpenPreflightWith::Initial(guard) = preflight else {
        panic!("exact initial authority must claim")
    };
    assert!(!coordinator.reservation_allowed());
    drop(guard);
    assert!(coordinator.reservation_allowed());

    let reservation_wins = picker_load_source_open_preflight_with(
        initial,
        || coordinator.try_begin_exclusive_with(&serialization, || true, || true),
        || initial,
    );
    assert!(matches!(
        reservation_wins,
        PickerLoadSourceOpenPreflightWith::Rejected(_)
    ));
    for rejected in [
        PickerLoadSourceOpenFacts {
            mode_active: true,
            ..initial
        },
        PickerLoadSourceOpenFacts {
            profile_owner: 0x5000,
            ..initial
        },
        PickerLoadSourceOpenFacts {
            exact_parent_authority: false,
            ..initial
        },
    ] {
        assert_eq!(
            classify_picker_load_source_open(rejected),
            PickerLoadSourceOpenDecision::Rejected
        );
    }
}

#[test]
fn rebuilt_or_visible_system_rows_remain_inert_while_picker_owns_input() {
    for facts in [
        PickerSystemRowInputFacts {
            quit_row_controller: true,
            picker_mode_active: true,
            transition_owned: false,
            system_rows_rebuilt: true,
            real_windows_hidden: false,
        },
        PickerSystemRowInputFacts {
            quit_row_controller: true,
            picker_mode_active: false,
            transition_owned: true,
            system_rows_rebuilt: true,
            real_windows_hidden: false,
        },
        PickerSystemRowInputFacts {
            quit_row_controller: true,
            picker_mode_active: true,
            transition_owned: true,
            system_rows_rebuilt: false,
            real_windows_hidden: true,
        },
    ] {
        assert!(picker_system_row_activation_is_inert(facts));
    }
    assert!(!picker_system_row_activation_is_inert(
        PickerSystemRowInputFacts {
            quit_row_controller: false,
            picker_mode_active: true,
            transition_owned: true,
            system_rows_rebuilt: true,
            real_windows_hidden: false,
        }
    ));
    assert!(!picker_system_row_activation_is_inert(
        PickerSystemRowInputFacts {
            quit_row_controller: true,
            picker_mode_active: false,
            transition_owned: false,
            system_rows_rebuilt: true,
            real_windows_hidden: false,
        }
    ));
}
