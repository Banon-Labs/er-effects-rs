use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InjectedFailure {
    NoStartDir,
    Model,
    Stage,
    Publish,
    NativeOpen,
}

#[derive(Debug, Default)]
struct TestOpenState {
    next_generation: u64,
    generation: u64,
    armed: bool,
    original_bytes: usize,
    model: usize,
    mode: usize,
    system: usize,
    action: usize,
    latches: usize,
    presentation_rows: usize,
    foreign_previews: usize,
}

struct TestOpenAttempt {
    state: std::sync::Arc<std::sync::Mutex<TestOpenState>>,
    generation: u64,
    committed: bool,
}

impl TestOpenAttempt {
    fn publish(&mut self, model: usize, succeed: bool) -> bool {
        let mut state = self.state.lock().unwrap();
        state.model = model;
        state.mode = 1;
        state.system = 0x7000;
        state.action = 0x7170;
        state.latches = 7;
        succeed
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for TestOpenAttempt {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.armed && state.generation == self.generation {
            let next_generation = state.next_generation;
            *state = TestOpenState {
                next_generation,
                ..TestOpenState::default()
            };
        }
    }
}

fn arm_test_attempt(state: &std::sync::Arc<std::sync::Mutex<TestOpenState>>) -> TestOpenAttempt {
    let generation = {
        let mut state = state.lock().unwrap();
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        state.generation = state.next_generation;
        state.armed = true;
        state.original_bytes = 128;
        state.generation
    };
    TestOpenAttempt {
        state: state.clone(),
        generation,
        committed: false,
    }
}

fn poll_test_preview(state: &std::sync::Arc<std::sync::Mutex<TestOpenState>>) {
    let mut state = state.lock().unwrap();
    if state.armed {
        state.foreign_previews += 1;
        state.presentation_rows += 10;
    }
}

fn run_injected_failure(failure: InjectedFailure) -> TestOpenState {
    let state = std::sync::Arc::new(std::sync::Mutex::new(TestOpenState::default()));
    let succeeded = execute_picker_initial_open_sequence_with(
        || Some(arm_test_attempt(&state)),
        || (failure != InjectedFailure::NoStartDir).then_some(0x5000usize),
        |_| (failure != InjectedFailure::Model).then_some(0x6000usize),
        |_| {
            if failure == InjectedFailure::Stage {
                state.lock().unwrap().presentation_rows = 4;
                false
            } else {
                true
            }
        },
        |attempt, model| attempt.publish(model, failure != InjectedFailure::Publish),
        || failure != InjectedFailure::NativeOpen,
        TestOpenAttempt::commit,
    );
    assert!(!succeeded);
    poll_test_preview(&state);
    std::sync::Arc::try_unwrap(state)
        .unwrap()
        .into_inner()
        .unwrap()
}

#[test]
fn every_post_arm_initial_open_failure_retires_arm_and_all_picker_state() {
    for failure in [
        InjectedFailure::NoStartDir,
        InjectedFailure::Model,
        InjectedFailure::Stage,
        InjectedFailure::Publish,
        InjectedFailure::NativeOpen,
    ] {
        let state = run_injected_failure(failure);
        assert!(!state.armed, "{failure:?}");
        assert_eq!(state.generation, 0, "{failure:?}");
        assert_eq!(state.original_bytes, 0, "{failure:?}");
        assert_eq!(state.model, 0, "{failure:?}");
        assert_eq!(state.mode, 0, "{failure:?}");
        assert_eq!(state.system, 0, "{failure:?}");
        assert_eq!(state.action, 0, "{failure:?}");
        assert_eq!(state.latches, 0, "{failure:?}");
        assert_eq!(state.presentation_rows, 0, "{failure:?}");
        assert_eq!(state.foreign_previews, 0, "{failure:?}");
    }
}

#[test]
fn successful_initial_open_transfers_exactly_one_current_arm() {
    let state = std::sync::Arc::new(std::sync::Mutex::new(TestOpenState::default()));
    assert!(execute_picker_initial_open_sequence_with(
        || Some(arm_test_attempt(&state)),
        || Some(0x5000usize),
        |_| Some(0x6000usize),
        |_| true,
        |attempt, model| attempt.publish(model, true),
        || true,
        TestOpenAttempt::commit,
    ));
    let state = state.lock().unwrap();
    assert!(state.armed);
    assert_eq!(state.generation, 1);
    assert_eq!(state.next_generation, 1);
    assert_eq!(state.original_bytes, 128);
    assert_eq!(state.model, 0x6000);
    assert_eq!(state.mode, 1);
    assert_eq!(state.system, 0x7000);
    assert_eq!(state.action, 0x7170);
    assert_eq!(state.latches, 7);
}

#[test]
fn initial_system_publication_and_rollback_are_exact_generation_aba_safe() {
    let coordinator = PickerSystemDialogCoordinator::default();
    let published = AtomicUsize::new(0);
    let first = coordinator
        .try_publish_initial_with(0x7000, |dialog| published.store(dialog, Ordering::SeqCst))
        .unwrap();
    assert_eq!(published.load(Ordering::SeqCst), 0x7000);
    assert!(
        coordinator
            .try_publish_initial_with(0x8000, |_| panic!("current identity blocks replacement"))
            .is_none()
    );
    assert!(!coordinator.clear_exact_with(
        PickerSystemDialogIdentity {
            generation: first.generation + 1,
            ..first
        },
        |_| panic!("stale generation must not clear")
    ));
    assert!(coordinator.clear_exact_with(first, |dialog| {
        published.store(dialog, Ordering::SeqCst)
    }));
    let second = coordinator
        .try_publish_initial_with(0x7000, |dialog| published.store(dialog, Ordering::SeqCst))
        .unwrap();
    assert_ne!(first.generation, second.generation);
    assert!(!coordinator.clear_exact_with(first, |_| {
        panic!("old same-address generation must not clear newer System identity")
    }));
    assert_eq!(coordinator.current_identity(), Some(second));
    assert_eq!(published.load(Ordering::SeqCst), 0x7000);
}

#[test]
fn exact_state_retirement_is_aba_safe_and_preserves_generation_source() {
    let state = std::sync::Mutex::new(SystemQuitSaveSwapState {
        next_generation: 1,
        arm_generation: 1,
        armed: true,
        path: "A".to_owned(),
        original_bytes: vec![1, 2, 3],
        original_hash: 0xaaaa,
        ..SystemQuitSaveSwapState::default()
    });
    let old = SystemQuitSaveSwapArmIdentity {
        generation: 1,
        path: "A".to_owned(),
        original_hash: 0xaaaa,
    };
    let retired = system_quit_save_swap_take_exact_with(&state, &old).unwrap();
    assert!(retired.armed);
    {
        let current = state.lock().unwrap();
        assert!(!system_quit_save_swap_poll_eligible(&current));
        assert_eq!(current.next_generation, 1);
        assert_eq!(current.arm_generation, 0);
        assert!(current.path.is_empty());
        assert!(current.original_bytes.is_empty());
    }
    {
        let mut current = state.lock().unwrap();
        current.next_generation = 2;
        current.arm_generation = 2;
        current.armed = true;
        current.path = "A".to_owned();
        current.original_bytes = vec![9, 9, 9];
        current.original_hash = 0xbbbb;
    }
    assert!(system_quit_save_swap_take_exact_with(&state, &old).is_none());
    let current = state.lock().unwrap();
    assert!(current.armed);
    assert_eq!(current.arm_generation, 2);
    assert_eq!(current.original_bytes, vec![9, 9, 9]);
    assert_eq!(current.original_hash, 0xbbbb);
}
