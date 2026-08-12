#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerPendingResubmitTransition {
    pub(crate) old_dialog: usize,
    pub(crate) system_dialog: usize,
    pub(crate) system_dialog_generation: usize,
    pub(crate) path_owner_generation: usize,
    pub(crate) refresh_owner_generation: usize,
    pub(crate) refresh_close_generation: usize,
    pub(crate) reopen_pending: usize,
    pub(crate) open_slots_pending: usize,
    pub(crate) resubmit_generation: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerPendingResubmitReservation {
    pub(crate) transition: PickerPendingResubmitTransition,
    reservation_generation: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerDestinationResubmitReservation {
    system_dialog: usize,
    system_dialog_generation: usize,
    reservation_generation: usize,
}

#[derive(Debug, Default)]
struct PickerPendingResubmitState {
    next_generation: usize,
    next_reservation_generation: usize,
    pending: Option<PickerPendingResubmitTransition>,
    reservation: Option<PickerPendingResubmitReservation>,
}

#[derive(Debug, Default)]
struct PickerDestinationResubmitState {
    next_reservation_generation: usize,
    reservation: Option<PickerDestinationResubmitReservation>,
}

#[derive(Debug, Default)]
struct PickerResetTransactionState {
    next_generation: usize,
    in_progress: Option<usize>,
    deferred: bool,
}

#[derive(Debug, Default)]
pub(crate) struct PickerResetTransactionCoordinator {
    state: std::sync::Mutex<PickerResetTransactionState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PickerDeferredResetAction {
    PickerState { source: String },
    RestoreRealWindows { base: usize, source: String },
}

#[derive(Debug)]
pub(crate) enum PickerResetBegin<'a> {
    Claimed(PickerResetTransactionGuard<'a>),
    Deferred { newly_recorded: bool },
    Coalesced,
}

#[derive(Debug)]
pub(crate) struct PickerResetTransactionGuard<'a> {
    serialization: &'a std::sync::Mutex<()>,
    coordinator: &'a PickerResetTransactionCoordinator,
    generation: usize,
    finished: bool,
}

impl PickerResetTransactionCoordinator {
    fn lock(&self) -> std::sync::MutexGuard<'_, PickerResetTransactionState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn reservation_allowed(&self) -> bool {
        let state = self.lock();
        state.in_progress.is_none() && !state.deferred
    }

    pub(crate) fn begin_with<'a>(
        &'a self,
        serialization: &'a std::sync::Mutex<()>,
        reservation_exists: impl FnOnce() -> bool,
    ) -> PickerResetBegin<'a> {
        self.begin_with_deferred(serialization, reservation_exists, |_| {})
    }

    pub(crate) fn begin_with_deferred<'a>(
        &'a self,
        serialization: &'a std::sync::Mutex<()>,
        reservation_exists: impl FnOnce() -> bool,
        record_deferred: impl FnOnce(bool),
    ) -> PickerResetBegin<'a> {
        let _serialization = serialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reservation_exists = reservation_exists();
        let mut state = self.lock();
        if reservation_exists {
            let newly_recorded = !state.deferred;
            state.deferred = true;
            record_deferred(newly_recorded);
            return PickerResetBegin::Deferred { newly_recorded };
        }
        if state.in_progress.is_some() {
            return PickerResetBegin::Coalesced;
        }
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        let generation = state.next_generation;
        state.in_progress = Some(generation);
        PickerResetBegin::Claimed(PickerResetTransactionGuard {
            serialization,
            coordinator: self,
            generation,
            finished: false,
        })
    }

    pub(crate) fn try_begin_exclusive_with<'a>(
        &'a self,
        serialization: &'a std::sync::Mutex<()>,
        reservation_exists: impl FnOnce() -> bool,
        authorized: impl FnOnce() -> bool,
    ) -> Option<PickerResetTransactionGuard<'a>> {
        let _serialization = serialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if reservation_exists() || !authorized() {
            return None;
        }
        let mut state = self.lock();
        if state.in_progress.is_some() || state.deferred {
            return None;
        }
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        let generation = state.next_generation;
        state.in_progress = Some(generation);
        Some(PickerResetTransactionGuard {
            serialization,
            coordinator: self,
            generation,
            finished: false,
        })
    }

    pub(crate) fn transaction_pending(&self) -> bool {
        let state = self.lock();
        state.in_progress.is_some() || state.deferred
    }

    pub(crate) fn claim_deferred_with<'a>(
        &'a self,
        serialization: &'a std::sync::Mutex<()>,
        reservation_exists: impl FnOnce() -> bool,
    ) -> Option<PickerResetTransactionGuard<'a>> {
        self.claim_deferred_with_action(serialization, reservation_exists, || ())
            .map(|(guard, ())| guard)
    }

    pub(crate) fn claim_deferred_with_action<'a, T>(
        &'a self,
        serialization: &'a std::sync::Mutex<()>,
        reservation_exists: impl FnOnce() -> bool,
        take_action: impl FnOnce() -> T,
    ) -> Option<(PickerResetTransactionGuard<'a>, T)> {
        let _serialization = serialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reservation_exists = reservation_exists();
        let mut state = self.lock();
        if reservation_exists || state.in_progress.is_some() || !state.deferred {
            return None;
        }
        state.deferred = false;
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        let generation = state.next_generation;
        state.in_progress = Some(generation);
        let action = take_action();
        Some((
            PickerResetTransactionGuard {
                serialization,
                coordinator: self,
                generation,
                finished: false,
            },
            action,
        ))
    }

    #[cfg(test)]
    fn snapshot(&self) -> (Option<usize>, bool) {
        let state = self.lock();
        (state.in_progress, state.deferred)
    }
}

impl PickerResetTransactionGuard<'_> {
    fn finish(&mut self) {
        if self.finished {
            return;
        }
        let _serialization = self
            .serialization
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self.coordinator.lock();
        if state.in_progress == Some(self.generation) {
            state.in_progress = None;
        }
        self.finished = true;
    }
}

impl Drop for PickerResetTransactionGuard<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

static SAVE_PICKER_RESET_TRANSACTION: PickerResetTransactionCoordinator =
    PickerResetTransactionCoordinator {
        state: std::sync::Mutex::new(PickerResetTransactionState {
            next_generation: 0,
            in_progress: None,
            deferred: false,
        }),
    };
static SAVE_PICKER_DEFERRED_RESET_ACTION: std::sync::Mutex<Option<PickerDeferredResetAction>> =
    std::sync::Mutex::new(None);
static SAVE_PICKER_DESTINATION_RESUBMIT_STATE: std::sync::Mutex<PickerDestinationResubmitState> =
    std::sync::Mutex::new(PickerDestinationResubmitState {
        next_reservation_generation: 0,
        reservation: None,
    });
static SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_RESUBMIT_LATCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn resubmit_latch_lock() -> std::sync::MutexGuard<'static, ()> {
    SAVE_PICKER_RESUBMIT_LATCH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn destination_resubmit_state() -> std::sync::MutexGuard<'static, PickerDestinationResubmitState> {
    SAVE_PICKER_DESTINATION_RESUBMIT_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn owner_resubmit_reserved() -> bool {
    pending_resubmit_state().reservation.is_some()
}

fn destination_resubmit_reserved() -> bool {
    destination_resubmit_state().reservation.is_some()
}

fn any_resubmit_reserved() -> bool {
    owner_resubmit_reserved() || destination_resubmit_reserved()
}

fn deferred_reset_action_lock() -> std::sync::MutexGuard<'static, Option<PickerDeferredResetAction>>
{
    SAVE_PICKER_DEFERRED_RESET_ACTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn record_deferred_reset_action_with(
    pending: &std::sync::Mutex<Option<PickerDeferredResetAction>>,
    action: PickerDeferredResetAction,
) {
    let mut pending = pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match (&*pending, &action) {
        (Some(PickerDeferredResetAction::RestoreRealWindows { .. }), _) => {}
        (_, PickerDeferredResetAction::RestoreRealWindows { .. }) | (None, _) => {
            *pending = Some(action);
        }
        (Some(PickerDeferredResetAction::PickerState { .. }), _) => {}
    }
}

fn record_deferred_reset_action(action: PickerDeferredResetAction) {
    record_deferred_reset_action_with(&SAVE_PICKER_DEFERRED_RESET_ACTION, action);
}

fn begin_picker_reset_action(action: PickerDeferredResetAction) -> PickerResetBegin<'static> {
    SAVE_PICKER_RESET_TRANSACTION.begin_with_deferred(
        &SAVE_PICKER_RESUBMIT_LATCH_LOCK,
        any_resubmit_reserved,
        |_| record_deferred_reset_action(action),
    )
}

pub(crate) fn begin_picker_state_reset_transaction(source: &str) -> PickerResetBegin<'static> {
    begin_picker_reset_action(PickerDeferredResetAction::PickerState {
        source: source.to_owned(),
    })
}

pub(crate) fn begin_picker_restore_reset_transaction(
    base: usize,
    source: &str,
) -> PickerResetBegin<'static> {
    begin_picker_reset_action(PickerDeferredResetAction::RestoreRealWindows {
        base,
        source: source.to_owned(),
    })
}

pub(crate) fn claim_deferred_picker_reset_transaction() -> Option<(
    PickerResetTransactionGuard<'static>,
    PickerDeferredResetAction,
)> {
    SAVE_PICKER_RESET_TRANSACTION.claim_deferred_with_action(
        &SAVE_PICKER_RESUBMIT_LATCH_LOCK,
        any_resubmit_reserved,
        || {
            deferred_reset_action_lock().take().unwrap_or_else(|| {
                PickerDeferredResetAction::PickerState {
                    source: "deferred-picker-reset".to_owned(),
                }
            })
        },
    )
}

fn picker_reservation_allowed() -> bool {
    SAVE_PICKER_RESET_TRANSACTION.reservation_allowed()
}

pub(crate) fn try_begin_picker_initial_open_transaction(
    authorized: impl FnOnce() -> bool,
) -> Option<PickerResetTransactionGuard<'static>> {
    SAVE_PICKER_RESET_TRANSACTION.try_begin_exclusive_with(
        &SAVE_PICKER_RESUBMIT_LATCH_LOCK,
        any_resubmit_reserved,
        authorized,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerSystemRowInputFacts {
    pub(crate) quit_row_controller: bool,
    pub(crate) picker_mode_active: bool,
    pub(crate) transition_owned: bool,
    // Diagnostic facts deliberately do not grant input authority.
    pub(crate) system_rows_rebuilt: bool,
    pub(crate) real_windows_hidden: bool,
}

pub(crate) fn picker_system_row_activation_is_inert(facts: PickerSystemRowInputFacts) -> bool {
    facts.quit_row_controller && (facts.picker_mode_active || facts.transition_owned)
}

/// Whether some picker transition currently owns the System rows. Shared by the input-inert gate and
/// the dead-mode release so they can never disagree about what "a transition is in flight" means.
/// Caller must hold `resubmit_latch_lock`.
fn picker_transition_owned_locked() -> bool {
    any_resubmit_reserved()
        || pending_resubmit_state().pending.is_some()
        || SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0
        || SAVE_PICKER_OPEN_SLOTS_PENDING.load(Ordering::SeqCst) != 0
        || SAVE_PICKER_REFRESH_PENDING_DIALOG.load(Ordering::SeqCst) != 0
        || er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG
            .load(Ordering::SeqCst)
            != 0
        || SAVE_PICKER_RESET_TRANSACTION.transaction_pending()
}

pub(crate) fn save_picker_system_rows_transition_owned() -> bool {
    let _guard = resubmit_latch_lock();
    picker_transition_owned_locked()
}

/// Identity of the currently-owning transition. Any real progress changes at least one of these, so
/// an unchanged signature across many pumps is the definition of a stalled transition.
///
/// EVERY field is bound to its own statement on purpose. Building this inline as one array literal
/// deadlocked the game on 2026-08-11: temporaries live until the end of the enclosing statement, so
/// the `pending_resubmit_state()` guard taken for the first element was still held when
/// `any_resubmit_reserved()` re-locked the same non-reentrant mutex for the last one. The menu pump
/// thread hung, the debug log froze mid-line, and the UI soft-locked with only the cursor moving.
pub(crate) fn save_picker_transition_signature() -> [usize; 6] {
    let _guard = resubmit_latch_lock();
    let resubmit_generation = pending_resubmit_state()
        .pending
        .map_or(0, |pending| pending.resubmit_generation);
    let reservations = usize::from(any_resubmit_reserved());
    let reset_pending = usize::from(SAVE_PICKER_RESET_TRANSACTION.transaction_pending());
    let reopen_pending = SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst);
    let open_slots_pending = SAVE_PICKER_OPEN_SLOTS_PENDING.load(Ordering::SeqCst);
    let refresh_dialog = SAVE_PICKER_REFRESH_PENDING_DIALOG.load(Ordering::SeqCst);
    let path_editor_dialog = er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG
        .load(Ordering::SeqCst);
    [
        resubmit_generation,
        reopen_pending,
        open_slots_pending,
        refresh_dialog,
        path_editor_dialog,
        reset_pending | reservations << 1,
    ]
}

pub(crate) fn save_picker_system_rows_input_inert() -> bool {
    let _guard = resubmit_latch_lock();
    let transition_owned = picker_transition_owned_locked();
    picker_system_row_activation_is_inert(PickerSystemRowInputFacts {
        quit_row_controller: true,
        picker_mode_active: SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
        transition_owned,
        system_rows_rebuilt: SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst) != 0,
        real_windows_hidden: SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0,
    })
}

pub(crate) fn save_picker_set_reopen_pending(value: usize) {
    let _guard = resubmit_latch_lock();
    if !any_resubmit_reserved() {
        SAVE_PICKER_REOPEN_PENDING.store(value, Ordering::SeqCst);
    }
}

pub(crate) fn save_picker_set_open_slots_pending(value: usize) {
    let _guard = resubmit_latch_lock();
    if !any_resubmit_reserved() {
        SAVE_PICKER_OPEN_SLOTS_PENDING.store(value, Ordering::SeqCst);
    }
}

static SAVE_PICKER_PENDING_RESUBMIT_STATE: std::sync::Mutex<PickerPendingResubmitState> =
    std::sync::Mutex::new(PickerPendingResubmitState {
        next_generation: 0,
        next_reservation_generation: 0,
        pending: None,
        reservation: None,
    });

fn pending_resubmit_state() -> std::sync::MutexGuard<'static, PickerPendingResubmitState> {
    SAVE_PICKER_PENDING_RESUBMIT_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn arm_picker_pending_resubmit_transition(
    old_dialog: usize,
    path_owner_generation: usize,
    refresh_owner_generation: usize,
) -> Option<PickerPendingResubmitTransition> {
    if old_dialog == 0 || (path_owner_generation == 0 && refresh_owner_generation == 0) {
        return None;
    }
    let system_identity = save_picker_system_dialog_identity()?;
    let refresh_close_generation =
        SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION.load(Ordering::SeqCst);
    let reopen_pending = SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst);
    let open_slots_pending = SAVE_PICKER_OPEN_SLOTS_PENDING.load(Ordering::SeqCst);
    if destination_resubmit_reserved() {
        return None;
    }
    let mut state = pending_resubmit_state();
    if state.reservation.is_some() {
        return None;
    }
    if state.pending.is_some_and(|pending| {
        pending.system_dialog != system_identity.dialog
            || pending.system_dialog_generation != system_identity.generation
    }) {
        return None;
    }
    let merged = state
        .pending
        .filter(|pending| pending.old_dialog == old_dialog)
        .map_or(
            (path_owner_generation, refresh_owner_generation),
            |pending| {
                (
                    path_owner_generation.max(pending.path_owner_generation),
                    refresh_owner_generation.max(pending.refresh_owner_generation),
                )
            },
        );
    if state.pending.is_some_and(|pending| {
        pending.old_dialog == old_dialog
            && pending.path_owner_generation == merged.0
            && pending.refresh_owner_generation == merged.1
            && pending.refresh_close_generation == refresh_close_generation
            && pending.reopen_pending == reopen_pending
            && pending.open_slots_pending == open_slots_pending
    }) {
        return state.pending;
    }
    state.next_generation = state.next_generation.wrapping_add(1).max(1);
    let transition = PickerPendingResubmitTransition {
        old_dialog,
        system_dialog: system_identity.dialog,
        system_dialog_generation: system_identity.generation,
        path_owner_generation: merged.0,
        refresh_owner_generation: merged.1,
        refresh_close_generation,
        reopen_pending,
        open_slots_pending,
        resubmit_generation: state.next_generation,
    };
    state.pending = Some(transition);
    Some(transition)
}

pub(crate) fn save_picker_pending_resubmit_transition() -> Option<PickerPendingResubmitTransition> {
    pending_resubmit_state().pending
}

pub(crate) fn picker_pending_resubmit_matches_native_removal_with(
    transition: Option<PickerPendingResubmitTransition>,
    reservation_active: bool,
    picker_mode_active: bool,
    system_identity: Option<PickerSystemDialogIdentity>,
    latches_match: bool,
    capture: PickerNativeRemovalCapture,
) -> bool {
    let Some(transition) = transition else {
        return false;
    };
    let generation_matches = transition.path_owner_generation == capture.owner.generation
        || transition.refresh_owner_generation == capture.owner.generation;
    let system_matches = system_identity.is_some_and(|identity| {
        identity.dialog == transition.system_dialog
            && identity.generation == transition.system_dialog_generation
    });
    !reservation_active
        && picker_mode_active
        && transition.old_dialog == capture.owner.dialog
        && generation_matches
        && system_matches
        && latches_match
}

pub(crate) fn save_picker_exact_pending_resubmit_for_native_removal(
    capture: PickerNativeRemovalCapture,
) -> Option<PickerPendingResubmitTransition> {
    with_picker_resubmit_latches(|| {
        let state = pending_resubmit_state();
        let transition = state.pending;
        let latches_match = transition.is_some_and(|transition| {
            picker_pending_resubmit_latches_match(
                &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG,
                &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION,
                &SAVE_PICKER_REFRESH_PENDING_DIALOG,
                &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
                &SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION,
                &SAVE_PICKER_REOPEN_PENDING,
                &SAVE_PICKER_OPEN_SLOTS_PENDING,
                transition,
            )
        });
        picker_pending_resubmit_matches_native_removal_with(
            transition,
            state.reservation.is_some(),
            SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
            save_picker_system_dialog_identity(),
            latches_match,
            capture,
        )
        .then_some(transition)
        .flatten()
    })
}

pub(crate) fn save_picker_pending_resubmit_matches_native_removal(
    capture: PickerNativeRemovalCapture,
) -> bool {
    save_picker_exact_pending_resubmit_for_native_removal(capture).is_some()
}

fn expected_resubmit_pairs(
    transition: PickerPendingResubmitTransition,
) -> ((usize, usize), (usize, usize)) {
    (
        if transition.path_owner_generation == 0 {
            (0, 0)
        } else {
            (transition.old_dialog, transition.path_owner_generation)
        },
        if transition.refresh_owner_generation == 0 {
            (0, 0)
        } else {
            (transition.old_dialog, transition.refresh_owner_generation)
        },
    )
}

fn picker_pending_resubmit_latches_match(
    path_dialog: &AtomicUsize,
    path_generation: &AtomicUsize,
    refresh_dialog: &AtomicUsize,
    refresh_generation: &AtomicUsize,
    refresh_close_generation: &AtomicUsize,
    reopen_pending: &AtomicUsize,
    open_slots_pending: &AtomicUsize,
    transition: PickerPendingResubmitTransition,
) -> bool {
    let (expected_path, expected_refresh) = expected_resubmit_pairs(transition);
    (
        path_dialog.load(Ordering::SeqCst),
        path_generation.load(Ordering::SeqCst),
    ) == expected_path
        && (
            refresh_dialog.load(Ordering::SeqCst),
            refresh_generation.load(Ordering::SeqCst),
        ) == expected_refresh
        && refresh_close_generation.load(Ordering::SeqCst) == transition.refresh_close_generation
        && reopen_pending.load(Ordering::SeqCst) == transition.reopen_pending
        && open_slots_pending.load(Ordering::SeqCst) == transition.open_slots_pending
}

fn reserve_picker_pending_resubmit_transition_with(
    state: &std::sync::Mutex<PickerPendingResubmitState>,
    path_dialog: &AtomicUsize,
    path_generation: &AtomicUsize,
    refresh_dialog: &AtomicUsize,
    refresh_generation: &AtomicUsize,
    refresh_close_generation: &AtomicUsize,
    reopen_pending: &AtomicUsize,
    open_slots_pending: &AtomicUsize,
    transition: PickerPendingResubmitTransition,
) -> Option<PickerPendingResubmitReservation> {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.pending != Some(transition)
        || state.reservation.is_some()
        || !picker_pending_resubmit_latches_match(
            path_dialog,
            path_generation,
            refresh_dialog,
            refresh_generation,
            refresh_close_generation,
            reopen_pending,
            open_slots_pending,
            transition,
        )
    {
        return None;
    }
    state.next_reservation_generation = state.next_reservation_generation.wrapping_add(1).max(1);
    let reservation = PickerPendingResubmitReservation {
        transition,
        reservation_generation: state.next_reservation_generation,
    };
    state.reservation = Some(reservation);
    Some(reservation)
}

fn commit_picker_pending_resubmit_reservation_with(
    state: &std::sync::Mutex<PickerPendingResubmitState>,
    path_dialog: &AtomicUsize,
    path_generation: &AtomicUsize,
    refresh_dialog: &AtomicUsize,
    refresh_generation: &AtomicUsize,
    refresh_close_generation: &AtomicUsize,
    reopen_pending: &AtomicUsize,
    open_slots_pending: &AtomicUsize,
    reservation: PickerPendingResubmitReservation,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let transition = reservation.transition;
    if state.reservation != Some(reservation)
        || state.pending != Some(transition)
        || !picker_pending_resubmit_latches_match(
            path_dialog,
            path_generation,
            refresh_dialog,
            refresh_generation,
            refresh_close_generation,
            reopen_pending,
            open_slots_pending,
            transition,
        )
    {
        return false;
    }
    path_dialog.store(0, Ordering::SeqCst);
    path_generation.store(0, Ordering::SeqCst);
    refresh_dialog.store(0, Ordering::SeqCst);
    refresh_generation.store(0, Ordering::SeqCst);
    refresh_close_generation.store(0, Ordering::SeqCst);
    reopen_pending.store(0, Ordering::SeqCst);
    open_slots_pending.store(0, Ordering::SeqCst);
    state.pending = None;
    state.reservation = None;
    true
}

fn release_picker_pending_resubmit_reservation_with(
    state: &std::sync::Mutex<PickerPendingResubmitState>,
    reservation: PickerPendingResubmitReservation,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.reservation != Some(reservation) {
        return false;
    }
    state.reservation = None;
    true
}

fn claim_picker_pending_resubmit_transition_with(
    state: &std::sync::Mutex<PickerPendingResubmitState>,
    path_dialog: &AtomicUsize,
    path_generation: &AtomicUsize,
    refresh_dialog: &AtomicUsize,
    refresh_generation: &AtomicUsize,
    refresh_close_generation: &AtomicUsize,
    reopen_pending: &AtomicUsize,
    open_slots_pending: &AtomicUsize,
    transition: PickerPendingResubmitTransition,
) -> bool {
    let Some(reservation) = reserve_picker_pending_resubmit_transition_with(
        state,
        path_dialog,
        path_generation,
        refresh_dialog,
        refresh_generation,
        refresh_close_generation,
        reopen_pending,
        open_slots_pending,
        transition,
    ) else {
        return false;
    };
    commit_picker_pending_resubmit_reservation_with(
        state,
        path_dialog,
        path_generation,
        refresh_dialog,
        refresh_generation,
        refresh_close_generation,
        reopen_pending,
        open_slots_pending,
        reservation,
    )
}

fn with_picker_resubmit_latches<R>(operation: impl FnOnce() -> R) -> R {
    let _latch_guard = resubmit_latch_lock();
    let _return_guard = path_editor_return_lock();
    let _refresh_guard = picker_refresh_state_lock();
    operation()
}

pub(crate) fn reserve_picker_pending_resubmit_transition(
    transition: PickerPendingResubmitTransition,
) -> Option<PickerPendingResubmitReservation> {
    with_picker_resubmit_latches(|| {
        if !picker_reservation_allowed() || destination_resubmit_reserved() {
            return None;
        }
        reserve_picker_pending_resubmit_transition_with(
            &SAVE_PICKER_PENDING_RESUBMIT_STATE,
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG,
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION,
            &SAVE_PICKER_REFRESH_PENDING_DIALOG,
            &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
            &SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION,
            &SAVE_PICKER_REOPEN_PENDING,
            &SAVE_PICKER_OPEN_SLOTS_PENDING,
            transition,
        )
    })
}

pub(crate) fn commit_picker_pending_resubmit_reservation(
    reservation: PickerPendingResubmitReservation,
) {
    let committed = with_picker_resubmit_latches(|| {
        commit_picker_pending_resubmit_reservation_with(
            &SAVE_PICKER_PENDING_RESUBMIT_STATE,
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG,
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION,
            &SAVE_PICKER_REFRESH_PENDING_DIALOG,
            &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
            &SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION,
            &SAVE_PICKER_REOPEN_PENDING,
            &SAVE_PICKER_OPEN_SLOTS_PENDING,
            reservation,
        )
    });
    debug_assert!(
        committed,
        "exclusive picker reservation must commit exactly"
    );
}

pub(crate) fn release_picker_pending_resubmit_reservation(
    reservation: PickerPendingResubmitReservation,
) -> bool {
    let _latch_guard = resubmit_latch_lock();
    release_picker_pending_resubmit_reservation_with(
        &SAVE_PICKER_PENDING_RESUBMIT_STATE,
        reservation,
    )
}

fn reserve_picker_destination_resubmit_transition_with(
    state: &std::sync::Mutex<PickerDestinationResubmitState>,
    reopen_pending: &AtomicUsize,
    open_slots_pending: &AtomicUsize,
    system_identity: PickerSystemDialogIdentity,
) -> Option<PickerDestinationResubmitReservation> {
    if reopen_pending.load(Ordering::SeqCst) != 0 || open_slots_pending.load(Ordering::SeqCst) != 1
    {
        return None;
    }
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.reservation.is_some() {
        return None;
    }
    state.next_reservation_generation = state.next_reservation_generation.wrapping_add(1).max(1);
    let reservation = PickerDestinationResubmitReservation {
        system_dialog: system_identity.dialog,
        system_dialog_generation: system_identity.generation,
        reservation_generation: state.next_reservation_generation,
    };
    state.reservation = Some(reservation);
    Some(reservation)
}

fn commit_picker_destination_resubmit_reservation_with(
    state: &std::sync::Mutex<PickerDestinationResubmitState>,
    reopen_pending: &AtomicUsize,
    open_slots_pending: &AtomicUsize,
    reservation: PickerDestinationResubmitReservation,
) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    debug_assert_eq!(state.reservation, Some(reservation));
    debug_assert_eq!(reopen_pending.load(Ordering::SeqCst), 0);
    debug_assert_eq!(open_slots_pending.load(Ordering::SeqCst), 1);
    open_slots_pending.store(0, Ordering::SeqCst);
    state.reservation = None;
}

fn release_picker_destination_resubmit_reservation_with(
    state: &std::sync::Mutex<PickerDestinationResubmitState>,
    reservation: PickerDestinationResubmitReservation,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.reservation != Some(reservation) {
        return false;
    }
    state.reservation = None;
    true
}

pub(crate) fn reserve_picker_destination_resubmit_transition(
    system_identity: PickerSystemDialogIdentity,
) -> Option<PickerDestinationResubmitReservation> {
    let _latch_guard = resubmit_latch_lock();
    if !picker_reservation_allowed() || owner_resubmit_reserved() {
        return None;
    }
    reserve_picker_destination_resubmit_transition_with(
        &SAVE_PICKER_DESTINATION_RESUBMIT_STATE,
        &SAVE_PICKER_REOPEN_PENDING,
        &SAVE_PICKER_OPEN_SLOTS_PENDING,
        system_identity,
    )
}

pub(crate) fn commit_picker_destination_resubmit_reservation(
    reservation: PickerDestinationResubmitReservation,
) {
    let _latch_guard = resubmit_latch_lock();
    commit_picker_destination_resubmit_reservation_with(
        &SAVE_PICKER_DESTINATION_RESUBMIT_STATE,
        &SAVE_PICKER_REOPEN_PENDING,
        &SAVE_PICKER_OPEN_SLOTS_PENDING,
        reservation,
    );
}

pub(crate) fn release_picker_destination_resubmit_reservation(
    reservation: PickerDestinationResubmitReservation,
) -> bool {
    let _latch_guard = resubmit_latch_lock();
    release_picker_destination_resubmit_reservation_with(
        &SAVE_PICKER_DESTINATION_RESUBMIT_STATE,
        reservation,
    )
}

pub(crate) fn clear_picker_pending_resubmit_transition() {
    let _latch_guard = resubmit_latch_lock();
    if any_resubmit_reserved() {
        return;
    }
    let mut state = pending_resubmit_state();
    state.pending = None;
    state.reservation = None;
}

fn abandon_picker_pending_resubmit_with(
    state: &std::sync::Mutex<PickerPendingResubmitState>,
    path_dialog: &AtomicUsize,
    path_generation: &AtomicUsize,
    refresh_dialog: &AtomicUsize,
    refresh_generation: &AtomicUsize,
    refresh_close_generation: &AtomicUsize,
    reopen_pending: &AtomicUsize,
    open_slots_pending: &AtomicUsize,
    expected: Option<PickerPendingResubmitTransition>,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let current = state.pending;
    if expected.is_none() && current.is_some()
        || expected.is_some() && current.is_some() && current != expected
    {
        if let Some(expected) = expected
            && state
                .reservation
                .is_some_and(|reservation| reservation.transition == expected)
        {
            state.reservation = None;
            return true;
        }
        return false;
    }
    let Some(transition) = expected.or(current) else {
        let had_state = state.reservation.take().is_some()
            || path_dialog.load(Ordering::SeqCst) != 0
            || path_generation.load(Ordering::SeqCst) != 0
            || refresh_dialog.load(Ordering::SeqCst) != 0
            || refresh_generation.load(Ordering::SeqCst) != 0
            || refresh_close_generation.load(Ordering::SeqCst) != 0
            || reopen_pending.load(Ordering::SeqCst) != 0
            || open_slots_pending.load(Ordering::SeqCst) != 0;
        path_dialog.store(0, Ordering::SeqCst);
        path_generation.store(0, Ordering::SeqCst);
        refresh_dialog.store(0, Ordering::SeqCst);
        refresh_generation.store(0, Ordering::SeqCst);
        refresh_close_generation.store(0, Ordering::SeqCst);
        reopen_pending.store(0, Ordering::SeqCst);
        open_slots_pending.store(0, Ordering::SeqCst);
        return had_state;
    };
    let (expected_path, expected_refresh) = expected_resubmit_pairs(transition);
    let path_pair = (
        path_dialog.load(Ordering::SeqCst),
        path_generation.load(Ordering::SeqCst),
    );
    let refresh_pair = (
        refresh_dialog.load(Ordering::SeqCst),
        refresh_generation.load(Ordering::SeqCst),
    );
    let reservation_matches = state
        .reservation
        .is_some_and(|reservation| reservation.transition == transition);
    let had_exact_state = current == Some(transition)
        || reservation_matches
        || expected_path != (0, 0) && path_pair == expected_path
        || expected_refresh != (0, 0) && refresh_pair == expected_refresh
        || transition.refresh_close_generation != 0
            && refresh_close_generation.load(Ordering::SeqCst)
                == transition.refresh_close_generation
        || transition.reopen_pending != 0
            && reopen_pending.load(Ordering::SeqCst) == transition.reopen_pending
        || transition.open_slots_pending != 0
            && open_slots_pending.load(Ordering::SeqCst) == transition.open_slots_pending;
    if !had_exact_state {
        return false;
    }
    let preserved_newer = (path_pair != expected_path && path_pair != (0, 0))
        || (refresh_pair != expected_refresh && refresh_pair != (0, 0));
    if path_pair == expected_path {
        path_dialog.store(0, Ordering::SeqCst);
        path_generation.store(0, Ordering::SeqCst);
    }
    if refresh_pair == expected_refresh {
        refresh_dialog.store(0, Ordering::SeqCst);
        refresh_generation.store(0, Ordering::SeqCst);
    }
    if refresh_close_generation.load(Ordering::SeqCst) == transition.refresh_close_generation {
        refresh_close_generation.store(0, Ordering::SeqCst);
    }
    if !preserved_newer {
        if reopen_pending.load(Ordering::SeqCst) == transition.reopen_pending {
            reopen_pending.store(0, Ordering::SeqCst);
        }
        if open_slots_pending.load(Ordering::SeqCst) == transition.open_slots_pending {
            open_slots_pending.store(0, Ordering::SeqCst);
        }
    }
    state.pending = None;
    if reservation_matches {
        state.reservation = None;
    }
    true
}

fn abandon_lost_system_dialog_resubmit_with(
    system_dialog: usize,
    expected: Option<PickerPendingResubmitTransition>,
    abandon: impl FnOnce(Option<PickerPendingResubmitTransition>) -> bool,
) -> Option<bool> {
    (system_dialog == 0).then(|| abandon(expected))
}

pub(crate) fn abandon_picker_pending_resubmit_for_system_dialog_loss(
    expected: Option<PickerPendingResubmitTransition>,
) -> bool {
    with_picker_resubmit_latches(|| {
        if any_resubmit_reserved() {
            return false;
        }
        abandon_picker_pending_resubmit_with(
            &SAVE_PICKER_PENDING_RESUBMIT_STATE,
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG,
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION,
            &SAVE_PICKER_REFRESH_PENDING_DIALOG,
            &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
            &SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION,
            &SAVE_PICKER_REOPEN_PENDING,
            &SAVE_PICKER_OPEN_SLOTS_PENDING,
            expected,
        )
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PickerRefreshRequest {
    dialog: usize,
    generation: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerRefreshRequestDisposition {
    Queued(PickerRefreshRequest),
    Coalesced(Option<PickerRefreshRequest>),
    Rejected,
}

static SAVE_PICKER_REFRESH_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn queue_picker_refresh_request_with(
    pending_dialog: &AtomicUsize,
    pending_generation: &AtomicUsize,
    _reopen_pending: bool,
    dialog: usize,
    next_generation: impl FnOnce() -> usize,
) -> PickerRefreshRequestDisposition {
    if dialog == 0 {
        return PickerRefreshRequestDisposition::Rejected;
    }
    let existing_dialog = pending_dialog.load(Ordering::SeqCst);
    let existing_generation = pending_generation.load(Ordering::SeqCst);
    let existing =
        (existing_dialog != 0 && existing_generation != 0).then_some(PickerRefreshRequest {
            dialog: existing_dialog,
            generation: existing_generation,
        });
    // A no-close path-editor return also sets the reopen latch. It must not coalesce away a
    // changed model/status marker: queue one exact content generation when none exists yet.
    if let Some(existing) = existing {
        return if existing.dialog == dialog {
            PickerRefreshRequestDisposition::Coalesced(Some(existing))
        } else {
            PickerRefreshRequestDisposition::Rejected
        };
    }
    if existing_dialog != 0 || existing_generation != 0 {
        return PickerRefreshRequestDisposition::Rejected;
    }
    let request = PickerRefreshRequest {
        dialog,
        generation: next_generation(),
    };
    // Publish generation first and dialog last. The production lock makes the pair indivisible to
    // peer producers/retirers; dialog remains the externally observed publication flag.
    pending_generation.store(request.generation, Ordering::SeqCst);
    pending_dialog.store(request.dialog, Ordering::SeqCst);
    PickerRefreshRequestDisposition::Queued(request)
}

fn load_picker_refresh_request_with(
    pending_dialog: &AtomicUsize,
    pending_generation: &AtomicUsize,
) -> Option<PickerRefreshRequest> {
    let dialog = pending_dialog.load(Ordering::SeqCst);
    let generation = pending_generation.load(Ordering::SeqCst);
    (dialog != 0 && generation != 0).then_some(PickerRefreshRequest { dialog, generation })
}

fn retire_picker_refresh_request_with(
    pending_dialog: &AtomicUsize,
    pending_generation: &AtomicUsize,
    reopen_pending: &AtomicUsize,
    request: PickerRefreshRequest,
    keep_reopen: bool,
) -> bool {
    if load_picker_refresh_request_with(pending_dialog, pending_generation) != Some(request) {
        return false;
    }
    pending_dialog.store(0, Ordering::SeqCst);
    pending_generation.store(0, Ordering::SeqCst);
    if !keep_reopen {
        reopen_pending.store(0, Ordering::SeqCst);
    }
    true
}

fn picker_refresh_state_lock() -> std::sync::MutexGuard<'static, ()> {
    SAVE_PICKER_REFRESH_STATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn load_picker_refresh_request() -> Option<PickerRefreshRequest> {
    let _guard = picker_refresh_state_lock();
    load_picker_refresh_request_with(
        &SAVE_PICKER_REFRESH_PENDING_DIALOG,
        &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
    )
}

fn retire_picker_refresh_request(request: PickerRefreshRequest, keep_reopen: bool) -> bool {
    let _latch_guard = resubmit_latch_lock();
    if any_resubmit_reserved() {
        return false;
    }
    let keep_reopen = keep_reopen || load_path_editor_return_reopen_request().is_some();
    let _guard = picker_refresh_state_lock();
    let retired = retire_picker_refresh_request_with(
        &SAVE_PICKER_REFRESH_PENDING_DIALOG,
        &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
        &SAVE_PICKER_REOPEN_PENDING,
        request,
        keep_reopen,
    );
    if retired {
        let _ = SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION.compare_exchange(
            request.generation,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    retired
}

fn clear_picker_refresh_request() {
    let _latch_guard = resubmit_latch_lock();
    if any_resubmit_reserved() {
        return;
    }
    let _guard = picker_refresh_state_lock();
    SAVE_PICKER_REFRESH_PENDING_DIALOG.store(0, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION.store(0, Ordering::SeqCst);
    SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION.store(0, Ordering::SeqCst);
}

/// Queue a fresh-owner presentation for one exact current picker dialog. This is only an atomic
/// request; ProfileSummary remains untouched until the old owner is observed as zero.
pub(crate) fn save_picker_schedule_refresh_request(dialog: usize, reason: &str) -> bool {
    let current = save_picker_live_profile_dialog();
    if dialog == 0 || dialog != current || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "save-picker: refresh request REJECTED stale identity requested=0x{dialog:x} current=0x{current:x} reason={reason}"
        ));
        return false;
    }
    er_telemetry::counters::SAVE_PICKER_REFRESH_REQUESTS.fetch_add(1, Ordering::SeqCst);
    let disposition = {
        // Reservation is nonblocking: writers reject instead of waiting or mutating the exact
        // retry transition while native submission is in flight.
        let _latch_guard = resubmit_latch_lock();
        if any_resubmit_reserved() {
            PickerRefreshRequestDisposition::Rejected
        } else {
            let _return_guard = path_editor_return_lock();
            let _guard = picker_refresh_state_lock();
            queue_picker_refresh_request_with(
                &SAVE_PICKER_REFRESH_PENDING_DIALOG,
                &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
                SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0,
                dialog,
                || {
                    er_telemetry::counters::SAVE_PICKER_REFRESH_GENERATION
                        .fetch_add(1, Ordering::SeqCst)
                        + 1
                },
            )
        }
    };
    match disposition {
        PickerRefreshRequestDisposition::Queued(request) => {
            er_telemetry::counters::SAVE_PICKER_REFRESH_LAST_OLD_OWNER
                .store(dialog, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker: queued fresh-owner refresh generation={} old_owner=0x{dialog:x} reason={reason}",
                request.generation
            ));
            true
        }
        PickerRefreshRequestDisposition::Coalesced(request) => {
            let generation = request.map_or_else(
                || er_telemetry::counters::SAVE_PICKER_REFRESH_GENERATION.load(Ordering::SeqCst),
                |request| request.generation,
            );
            let coalesces = er_telemetry::counters::SAVE_PICKER_REFRESH_COALESCES
                .fetch_add(1, Ordering::SeqCst)
                + 1;
            append_autoload_debug(format_args!(
                "save-picker: coalesced refresh request generation={generation} old_owner=0x{dialog:x} reason={reason} coalesces={coalesces}"
            ));
            true
        }
        PickerRefreshRequestDisposition::Rejected => {
            append_autoload_debug(format_args!(
                "save-picker: refresh request REJECTED conflicting pending owner requested=0x{dialog:x} reason={reason}"
            ));
            false
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PathEditorReturnReopenRequest {
    dialog: usize,
    generation: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathEditorReturnReopenDisposition {
    Queued(PathEditorReturnReopenRequest),
    Coalesced(PathEditorReturnReopenRequest),
    Rejected,
}

static SAVE_PICKER_PATH_EDITOR_RETURN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn path_editor_return_lock() -> std::sync::MutexGuard<'static, ()> {
    SAVE_PICKER_PATH_EDITOR_RETURN_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn load_path_editor_return_reopen_request_with(
    pending_dialog: &AtomicUsize,
    pending_generation: &AtomicUsize,
) -> Option<PathEditorReturnReopenRequest> {
    let dialog = pending_dialog.load(Ordering::SeqCst);
    let generation = pending_generation.load(Ordering::SeqCst);
    (dialog != 0 && generation != 0).then_some(PathEditorReturnReopenRequest { dialog, generation })
}

fn queue_path_editor_return_reopen_with(
    pending_dialog: &AtomicUsize,
    pending_generation: &AtomicUsize,
    reopen_pending: &AtomicUsize,
    request: PathEditorReturnReopenRequest,
) -> PathEditorReturnReopenDisposition {
    if request.dialog == 0 || request.generation == 0 {
        return PathEditorReturnReopenDisposition::Rejected;
    }
    if let Some(existing) =
        load_path_editor_return_reopen_request_with(pending_dialog, pending_generation)
    {
        return if existing == request {
            PathEditorReturnReopenDisposition::Coalesced(existing)
        } else {
            PathEditorReturnReopenDisposition::Rejected
        };
    }
    if pending_dialog.load(Ordering::SeqCst) != 0 || pending_generation.load(Ordering::SeqCst) != 0
    {
        return PathEditorReturnReopenDisposition::Rejected;
    }
    pending_generation.store(request.generation, Ordering::SeqCst);
    pending_dialog.store(request.dialog, Ordering::SeqCst);
    reopen_pending.store(1, Ordering::SeqCst);
    PathEditorReturnReopenDisposition::Queued(request)
}

fn load_path_editor_return_reopen_request() -> Option<PathEditorReturnReopenRequest> {
    let _guard = path_editor_return_lock();
    load_path_editor_return_reopen_request_with(
        &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG,
        &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION,
    )
}

pub(crate) fn save_picker_path_editor_return_pending_for(dialog: usize) -> bool {
    dialog != 0
        && load_path_editor_return_reopen_request()
            .is_some_and(|request| request.dialog == dialog && request.generation != 0)
}

fn clear_path_editor_return_reopen_request() {
    let _latch_guard = resubmit_latch_lock();
    if any_resubmit_reserved() {
        return;
    }
    let _guard = path_editor_return_lock();
    er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG
        .store(0, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION
        .store(0, Ordering::SeqCst);
}

fn path_editor_return_matches_owner(
    request: PathEditorReturnReopenRequest,
    owner_dialog: usize,
    owner_generation: usize,
) -> bool {
    request.dialog == owner_dialog && request.generation == owner_generation
}

pub(crate) fn save_picker_reconcile_path_editor_return_owner(
    new_dialog: usize,
    lifecycle_generation: u64,
) {
    if new_dialog == 0 {
        return;
    }
    let Ok(lifecycle_generation) = usize::try_from(lifecycle_generation) else {
        return;
    };
    let _latch_guard = resubmit_latch_lock();
    if any_resubmit_reserved() {
        return;
    }
    let _return_guard = path_editor_return_lock();
    let Some(request) = load_path_editor_return_reopen_request_with(
        &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG,
        &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION,
    ) else {
        return;
    };
    if path_editor_return_matches_owner(request, new_dialog, lifecycle_generation) {
        return;
    }
    let _refresh_guard = picker_refresh_state_lock();
    er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG
        .store(0, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION
        .store(0, Ordering::SeqCst);
    if load_picker_refresh_request_with(
        &SAVE_PICKER_REFRESH_PENDING_DIALOG,
        &er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_GENERATION,
    )
    .is_none()
    {
        SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "save-picker-path: retired stale parent-return request dialog=0x{:x} generation={} for newer owner=0x{new_dialog:x} generation={lifecycle_generation}",
        request.dialog, request.generation
    ));
}

fn save_picker_arm_path_editor_return_reopen_validated(
    ticket: er_save_picker::PathEditorRequestTicket,
    reason: &str,
) -> bool {
    let Ok(generation) = usize::try_from(ticket.generation) else {
        return false;
    };
    let _latch_guard = resubmit_latch_lock();
    let disposition = if any_resubmit_reserved() {
        PathEditorReturnReopenDisposition::Rejected
    } else {
        let _guard = path_editor_return_lock();
        queue_path_editor_return_reopen_with(
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG,
            &er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_GENERATION,
            &SAVE_PICKER_REOPEN_PENDING,
            PathEditorReturnReopenRequest {
                dialog: ticket.dialog,
                generation,
            },
        )
    };
    match disposition {
        PathEditorReturnReopenDisposition::Queued(request) => {
            let _ = arm_picker_pending_resubmit_transition(request.dialog, request.generation, 0);
            er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_REQUESTS
                .fetch_add(1, Ordering::SeqCst);
            er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_NO_CLOSE_REOPENS
                .fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-path: armed no-close parent return reopen dialog=0x{:x} generation={} reason={reason}; waiting for native owner disappearance",
                request.dialog, request.generation
            ));
            true
        }
        PathEditorReturnReopenDisposition::Coalesced(request) => {
            let _ = arm_picker_pending_resubmit_transition(request.dialog, request.generation, 0);
            er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_COALESCES
                .fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-path: coalesced no-close parent return reopen dialog=0x{:x} generation={} reason={reason}",
                request.dialog, request.generation
            ));
            true
        }
        PathEditorReturnReopenDisposition::Rejected => {
            append_autoload_debug(format_args!(
                "save-picker-path: return reopen REJECTED conflicting generation dialog=0x{:x} generation={} reason={reason}",
                ticket.dialog, ticket.generation
            ));
            false
        }
    }
}

pub(crate) fn save_picker_arm_path_editor_return_reopen(
    ticket: er_save_picker::PathEditorRequestTicket,
    reason: &str,
) -> bool {
    let current = save_picker_live_profile_dialog();
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0
        || (current != 0
            && (current != ticket.dialog
                || !save_picker_path_editor_ticket_matches_current_owner(ticket)))
    {
        append_autoload_debug(format_args!(
            "save-picker-path: return reopen REJECTED dialog=0x{:x} generation={} current=0x{current:x} reason={reason}",
            ticket.dialog, ticket.generation
        ));
        return false;
    }
    save_picker_arm_path_editor_return_reopen_validated(ticket, reason)
}

/// Called only inside `PathEditorCoordinator::with_terminal_result_transaction`, whose lifecycle
/// mutex excludes every owner publication. Lock order continues lifecycle -> model -> return state;
/// this helper must not re-enter lifecycle or invoke native code.
pub(crate) fn save_picker_arm_path_editor_return_reopen_transaction_owned(
    ticket: er_save_picker::PathEditorRequestTicket,
    transaction_identity: er_save_picker::PathEditorPickerIdentity,
    reason: &str,
) -> bool {
    if !transaction_identity.picker_mode_active
        || (transaction_identity.current_dialog != 0
            && transaction_identity.current_dialog != ticket.dialog)
    {
        return false;
    }
    save_picker_arm_path_editor_return_reopen_validated(ticket, reason)
}
