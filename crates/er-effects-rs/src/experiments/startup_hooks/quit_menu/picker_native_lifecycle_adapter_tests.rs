use super::*;
use std::cell::{Cell, RefCell};

const TEST_DIALOG: usize = 0x5000;
const TEST_VTABLE: usize = 0x6000;
const CALLBACK_EXACT: usize = 0;
const CALLBACK_DIALOG_MISMATCH: usize = 1;

std::thread_local! {
    static TEST_CALLBACKS: Cell<usize> = const { Cell::new(0) };
    static TEST_CALLBACK_IDENTITY: Cell<usize> = const { Cell::new(CALLBACK_EXACT) };
    static TEST_UPDATE_ORIGINALS: Cell<usize> = const { Cell::new(0) };
    static TEST_PROFILE_ORIGINALS: Cell<usize> = const { Cell::new(0) };
    static TEST_EFFECT_CALLS: Cell<usize> = const { Cell::new(0) };
    static TEST_TELEMETRY: RefCell<Vec<PickerActivationContext>> = const { RefCell::new(Vec::new()) };
}

fn exact_identity(dialog: usize) -> PickerDialogIdentity {
    PickerDialogIdentity {
        picker_mode_active: true,
        dialog,
        active_dialog: dialog,
        expected_vtable: Some(TEST_VTABLE),
        actual_vtable: Some(TEST_VTABLE),
    }
}

unsafe extern "system" fn test_profile_original(_: usize) {
    TEST_PROFILE_ORIGINALS.with(|count| count.set(count.get() + 1));
}

unsafe fn test_effect_sink(
    _: usize,
    _: i32,
    provenance: er_save_picker::DriveStripActivationProvenance,
) -> er_save_picker::PickerSourceDecision {
    TEST_EFFECT_CALLS.with(|count| count.set(count.get() + 1));
    match provenance {
        er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept => {
            er_save_picker::PickerSourceDecision::Effect(
                er_save_picker::PickerNativeActivationEffect::RequestPathEditor,
            )
        }
        er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(
            er_save_picker::DriveStripFocus::Cell(cell),
        ) => er_save_picker::PickerSourceDecision::Effect(
            er_save_picker::PickerNativeActivationEffect::DriveSelected(cell),
        ),
        er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(
            er_save_picker::DriveStripFocus::CurrentPath,
        ) => er_save_picker::PickerSourceDecision::Effect(
            er_save_picker::PickerNativeActivationEffect::RequestPathEditor,
        ),
        er_save_picker::DriveStripActivationProvenance::OrdinaryRowPhysicalActivation => {
            er_save_picker::PickerSourceDecision::Effect(
                er_save_picker::PickerNativeActivationEffect::Model(
                    crate::experiments::save_picker::PickerActivation::Repopulate,
                ),
            )
        }
        er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick => {
            er_save_picker::PickerSourceDecision::Rejected(
                er_save_picker::PickerSourceRejection::RejectedPhysicalClick,
            )
        }
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation => {
            er_save_picker::PickerSourceDecision::Rejected(
                er_save_picker::PickerSourceRejection::UnknownSource,
            )
        }
    }
}

fn test_telemetry_sink(context: PickerActivationContext) {
    TEST_TELEMETRY.with(|records| records.borrow_mut().push(context));
}

fn test_adapter() -> PickerNativeLifecycleAdapter {
    PickerNativeLifecycleAdapter {
        update_original: Some(test_update_original),
        profile_load_original: Some(test_profile_original),
        effect_sink: test_effect_sink,
        telemetry_sink: test_telemetry_sink,
    }
}

unsafe extern "system" fn test_update_original(dialog: usize, _: f32, _: *const u8) {
    TEST_UPDATE_ORIGINALS.with(|count| count.set(count.get() + 1));
    let callbacks = TEST_CALLBACKS.with(Cell::get);
    for _ in 0..callbacks {
        let identity = if TEST_CALLBACK_IDENTITY.with(Cell::get) == CALLBACK_DIALOG_MISMATCH {
            PickerDialogIdentity {
                dialog: dialog + 8,
                active_dialog: dialog,
                ..exact_identity(dialog)
            }
        } else {
            exact_identity(dialog)
        };
        let _ = unsafe { test_adapter().dispatch_profile_load(identity, 0) };
    }
}

fn context(provenance: er_save_picker::DriveStripActivationProvenance) -> PickerActivationContext {
    PickerActivationContext {
        seq: SAVE_PICKER_ACTIVATION_SEQ.fetch_add(1, Ordering::SeqCst) + 1,
        source: "menu-window-update",
        dialog: TEST_DIALOG,
        row_input_gate: 0x7000,
        cursor: 0,
        model_row: Some(0),
        layout_generation: 1,
        layout_hash: 2,
        provenance,
        physical_click: None,
        callback_count: 0,
        route_count: 0,
        effect_count: 0,
        update_forward_count: 0,
        profile_load_original_count: 0,
        terminal_count: 0,
        route: "none",
        effect: "none",
        terminal: "none",
    }
}

fn reset(callbacks: usize, callback_identity: usize) {
    TEST_CALLBACKS.with(|value| value.set(callbacks));
    TEST_CALLBACK_IDENTITY.with(|value| value.set(callback_identity));
    TEST_UPDATE_ORIGINALS.with(|value| value.set(0));
    TEST_PROFILE_ORIGINALS.with(|value| value.set(0));
    TEST_EFFECT_CALLS.with(|value| value.set(0));
    TEST_TELEMETRY.with(|records| records.borrow_mut().clear());
    SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| {
        assert!(
            slot.replace(None).is_none(),
            "test inherited pending context"
        );
    });
}

fn run_update(provenance: er_save_picker::DriveStripActivationProvenance) {
    unsafe {
        test_adapter().run_update_with(
            exact_identity(TEST_DIALOG),
            1.0,
            0x7000 as *const u8,
            || context(provenance),
        );
    }
}

#[test]
fn synchronous_callback_is_suppressed_and_effected_before_update_finalization() {
    reset(1, CALLBACK_EXACT);
    run_update(er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept);
    assert_eq!(TEST_UPDATE_ORIGINALS.with(Cell::get), 1);
    assert_eq!(TEST_PROFILE_ORIGINALS.with(Cell::get), 0);
    assert_eq!(TEST_EFFECT_CALLS.with(Cell::get), 1);
    TEST_TELEMETRY.with(|records| {
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].callback_count, 1);
        assert_eq!(records[0].effect_count, 1);
        assert_eq!(records[0].terminal_count, 1);
        assert_eq!(records[0].terminal, "route-committed");
    });
    SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| assert!(slot.borrow().is_none()));
}

#[test]
fn known_no_callback_is_red_but_unknown_update_is_silent() {
    reset(0, CALLBACK_EXACT);
    run_update(er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept);
    TEST_TELEMETRY.with(|records| {
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].callback_count, 0);
        assert_eq!(records[0].terminal_count, 1);
        assert_eq!(records[0].terminal, "native-matcher-no-callback");
        assert_eq!(records[0].effect, "native-matcher-no-callback");
    });

    reset(0, CALLBACK_EXACT);
    run_update(er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation);
    assert!(TEST_TELEMETRY.with(|records| records.borrow().is_empty()));
    SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| assert!(slot.borrow().is_none()));
}

#[test]
fn identity_mismatches_and_missing_identity_forward_original_exactly_once() {
    reset(0, CALLBACK_EXACT);
    let identities = [
        PickerDialogIdentity {
            picker_mode_active: false,
            ..exact_identity(TEST_DIALOG)
        },
        PickerDialogIdentity {
            active_dialog: TEST_DIALOG + 8,
            ..exact_identity(TEST_DIALOG)
        },
        PickerDialogIdentity {
            expected_vtable: None,
            actual_vtable: None,
            ..exact_identity(TEST_DIALOG)
        },
        PickerDialogIdentity {
            actual_vtable: None,
            ..exact_identity(TEST_DIALOG)
        },
        PickerDialogIdentity {
            actual_vtable: Some(TEST_VTABLE + 8),
            ..exact_identity(TEST_DIALOG)
        },
    ];
    for (index, identity) in identities.into_iter().enumerate() {
        assert_eq!(
            unsafe { test_adapter().dispatch_profile_load(identity, 0) },
            PickerProfileLoadDispatch::OriginalForwarded
        );
        assert_eq!(TEST_PROFILE_ORIGINALS.with(Cell::get), index + 1);
    }
    assert!(TEST_TELEMETRY.with(|records| records.borrow().is_empty()));
}

#[test]
fn callback_identity_mismatch_forwards_and_leaves_a_missing_callback_terminal() {
    reset(1, CALLBACK_DIALOG_MISMATCH);
    run_update(er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept);
    assert_eq!(TEST_PROFILE_ORIGINALS.with(Cell::get), 1);
    assert_eq!(TEST_EFFECT_CALLS.with(Cell::get), 0);
    TEST_TELEMETRY.with(|records| {
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].terminal, "native-matcher-no-callback");
    });
}

#[test]
fn exact_late_callback_is_named_and_suppressed() {
    reset(0, CALLBACK_EXACT);
    assert_eq!(
        unsafe { test_adapter().dispatch_profile_load(exact_identity(TEST_DIALOG), 0) },
        PickerProfileLoadDispatch::PickerSuppressed
    );
    assert_eq!(TEST_PROFILE_ORIGINALS.with(Cell::get), 0);
    TEST_TELEMETRY.with(|records| {
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, "profile-load-late");
        assert_eq!(records[0].terminal, "late");
    });
}

#[test]
fn duplicate_callback_is_rejected_without_native_profile_load() {
    reset(2, CALLBACK_EXACT);
    run_update(er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept);
    assert_eq!(TEST_PROFILE_ORIGINALS.with(Cell::get), 0);
    assert_eq!(TEST_EFFECT_CALLS.with(Cell::get), 1);
    TEST_TELEMETRY.with(|records| {
        let records = records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].callback_count, 2);
        assert_eq!(records[0].route_count, 2);
        assert_eq!(records[0].terminal_count, 1);
        assert_eq!(records[0].terminal, "duplicate-callback");
    });
}

fn valid_committed_observation() -> PickerLifecycleInvariantObservation {
    let mut context = context(er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept);
    context.callback_count = 1;
    context.route_count = 1;
    context.effect_count = 1;
    context.update_forward_count = 1;
    context.terminal_count = 1;
    context.route = "path-editor";
    context.effect = "path-editor-requested";
    context.terminal = "route-committed";
    PickerLifecycleInvariantObservation {
        identity_exact: true,
        context_present_at_callback: true,
        telemetry_count: 1,
        context,
    }
}

#[test]
fn mutation_context_cleared_before_callback_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.context_present_at_callback = false;
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_CONTEXT_MISSING,
        0
    );
}

#[test]
fn mutation_callback_duplication_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.context.callback_count = 2;
    mutant.context.route_count = 2;
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_CALLBACK_DUPLICATED,
        0
    );
}

#[test]
fn mutation_effect_duplication_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.context.effect_count = 2;
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_EFFECT_INVALID,
        0
    );
}

#[test]
fn mutation_missing_effect_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.context.effect_count = 0;
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_EFFECT_INVALID,
        0
    );
}

#[test]
fn mutation_native_original_in_exact_picker_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.context.profile_load_original_count = 1;
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_NATIVE_ORIGINAL_IN_PICKER,
        0
    );
}

#[test]
fn mutation_missing_no_callback_terminal_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.context.callback_count = 0;
    mutant.context.route_count = 1;
    mutant.context.effect_count = 0;
    mutant.context.terminal_count = 0;
    mutant.context.effect = "none";
    mutant.context.terminal = "none";
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant)
            & PICKER_LIFECYCLE_NO_CALLBACK_TERMINAL_MISSING,
        0
    );
}

#[test]
fn mutation_wrong_late_label_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.context.source = "profile-load-late";
    mutant.context.effect_count = 0;
    mutant.context.effect = "wrong-late";
    mutant.context.terminal = "wrong-late";
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_LATE_LABEL_INVALID,
        0
    );
}

#[test]
fn mutation_identity_gate_removal_foreign_suppression_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.identity_exact = false;
    mutant.context.effect_count = 0;
    mutant.context.effect = "none";
    mutant.context.terminal = "forwarded";
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_FOREIGN_SUPPRESSED,
        0
    );
}

#[test]
fn mutation_telemetry_omitted_is_rejected() {
    let mut mutant = valid_committed_observation();
    mutant.telemetry_count = 0;
    assert_ne!(
        validate_picker_lifecycle_invariants(mutant) & PICKER_LIFECYCLE_TELEMETRY_INVALID,
        0
    );
}

#[test]
fn production_lifecycle_invariant_baseline_is_accepted() {
    assert_eq!(
        validate_picker_lifecycle_invariants(valid_committed_observation()),
        0
    );
}

#[test]
fn thirty_two_events_replay_through_the_production_lifecycle_adapter() {
    reset(1, CALLBACK_EXACT);
    for event in 0..32 {
        let provenance = match event % 4 {
            0 => er_save_picker::DriveStripActivationProvenance::OrdinaryRowPhysicalActivation,
            1 => er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(
                er_save_picker::DriveStripFocus::Cell(event % 3),
            ),
            2 => er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(
                er_save_picker::DriveStripFocus::CurrentPath,
            ),
            _ => er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick,
        };
        run_update(provenance);
    }
    assert_eq!(TEST_UPDATE_ORIGINALS.with(Cell::get), 32);
    assert_eq!(TEST_PROFILE_ORIGINALS.with(Cell::get), 0);
    assert_eq!(TEST_EFFECT_CALLS.with(Cell::get), 32);
    TEST_TELEMETRY.with(|records| {
        let records = records.borrow();
        assert_eq!(records.len(), 32);
        assert!(records.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        assert!(records.iter().all(|record| record.terminal_count == 1));
        assert!(records.iter().all(|record| record.callback_count == 1));
        assert!(
            records
                .iter()
                .all(|record| record.profile_load_original_count == 0)
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.effect_count)
                .sum::<usize>(),
            24
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record.route == "reject")
                .count(),
            8
        );
    });
    SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| assert!(slot.borrow().is_none()));
}
