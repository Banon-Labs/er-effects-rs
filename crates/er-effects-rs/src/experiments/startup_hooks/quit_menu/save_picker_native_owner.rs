#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOwnerLineage {
    pub(crate) dialog: usize,
    pub(crate) generation: usize,
    pub(crate) job: usize,
    pub(crate) job_lineage: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerRunRegistration {
    pub(crate) owner_generation: usize,
    pub(crate) job: usize,
    pub(crate) list: usize,
    pub(crate) job_lineage: usize,
    pub(crate) run_lineage: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerNativeRemovalCapture {
    pub(crate) owner: PickerOwnerLineage,
    pub(crate) run: PickerRunRegistration,
    pub(crate) list: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOwnerClearedLineage {
    pub(crate) old_owner: PickerOwnerLineage,
    pub(crate) old_run: PickerRunRegistration,
    pub(crate) zero_generation: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerNativeRemovalAuthority {
    pub(crate) pending: PickerPendingResubmitTransition,
    pub(crate) cleared: PickerOwnerClearedLineage,
    pub(crate) list: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerOwnerPublicationRequest {
    Set {
        new_dialog: usize,
        job: usize,
    },
    CompareSet {
        expected: usize,
        new_dialog: usize,
    },
    CompareRemove {
        expected: PickerNativeRemovalCapture,
        pending: PickerPendingResubmitTransition,
        new_dialog: usize,
    },
}

impl PickerOwnerPublicationRequest {
    fn new_dialog(self) -> usize {
        match self {
            Self::Set { new_dialog, .. }
            | Self::CompareSet { new_dialog, .. }
            | Self::CompareRemove { new_dialog, .. } => new_dialog,
        }
    }

    fn job(self) -> usize {
        match self {
            Self::Set { job, .. } => job,
            Self::CompareSet { .. } | Self::CompareRemove { .. } => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOwnerAppliedPublication {
    pub(crate) previous: usize,
    pub(crate) cancelled_close: Option<er_save_picker::PathEditorDeferredCloseTicket>,
    pub(crate) lifecycle_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerOwnerApplyResult {
    Published(PickerOwnerAppliedPublication),
    Stale { actual: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerOwnerPublicationDisposition {
    Published(PickerOwnerAppliedPublication),
    Stale { actual: usize },
    Deferred,
}

#[derive(Debug, Default)]
struct PickerOwnerLifetimeState {
    active_leases: usize,
    deferred: std::collections::VecDeque<PickerOwnerPublicationRequest>,
    current: Option<PickerOwnerLineage>,
    current_run: Option<PickerRunRegistration>,
    cleared: Option<PickerOwnerClearedLineage>,
    native_removal: Option<PickerNativeRemovalAuthority>,
    next_generation: usize,
    next_job_lineage: usize,
    next_run_lineage: usize,
    next_zero_generation: usize,
}

#[derive(Debug, Default)]
pub(crate) struct PickerOwnerLifetimeCoordinator {
    state: std::sync::Mutex<PickerOwnerLifetimeState>,
}

impl PickerOwnerLifetimeCoordinator {
    fn lock(&self) -> std::sync::MutexGuard<'_, PickerOwnerLifetimeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn apply_one(
        state: &mut PickerOwnerLifetimeState,
        request: PickerOwnerPublicationRequest,
        apply: impl FnOnce(PickerOwnerPublicationRequest) -> PickerOwnerApplyResult,
    ) -> PickerOwnerPublicationDisposition {
        if let PickerOwnerPublicationRequest::CompareRemove { expected, .. } = request
            && (state.current != Some(expected.owner) || state.current_run != Some(expected.run))
        {
            return PickerOwnerPublicationDisposition::Stale {
                actual: state.current.map_or(0, |owner| owner.dialog),
            };
        }
        let result = apply(request);
        let PickerOwnerApplyResult::Published(publication) = result else {
            let PickerOwnerApplyResult::Stale { actual } = result else {
                unreachable!()
            };
            return PickerOwnerPublicationDisposition::Stale { actual };
        };
        let new_dialog = request.new_dialog();
        let request_job = request.job();
        if new_dialog == 0 {
            if let Some((old_owner, old_run)) = state
                .current
                .filter(|owner| owner.dialog != 0)
                .zip(state.current_run)
                .filter(|(owner, run)| {
                    run.owner_generation == owner.generation
                        && run.job == owner.job
                        && run.job_lineage == owner.job_lineage
                })
            {
                state.next_zero_generation = state.next_zero_generation.wrapping_add(1).max(1);
                state.cleared = Some(PickerOwnerClearedLineage {
                    old_owner,
                    old_run,
                    zero_generation: state.next_zero_generation,
                });
            } else {
                state.cleared = None;
            }
            state.current = None;
            state.current_run = None;
            if let (
                PickerOwnerPublicationRequest::CompareRemove {
                    expected, pending, ..
                },
                Some(cleared),
            ) = (request, state.cleared)
            {
                state.native_removal = Some(PickerNativeRemovalAuthority {
                    pending,
                    cleared,
                    list: expected.list,
                });
            }
        } else {
            let same_lineage = request_job != 0
                && state
                    .current
                    .is_some_and(|owner| owner.dialog == new_dialog && owner.job == request_job);
            if !same_lineage {
                state.next_generation = state.next_generation.wrapping_add(1).max(1);
                state.next_job_lineage = state.next_job_lineage.wrapping_add(1).max(1);
                state.current_run = None;
            }
            let previous = state.current;
            state.current = Some(PickerOwnerLineage {
                dialog: new_dialog,
                generation: if same_lineage {
                    previous.map_or(state.next_generation, |owner| owner.generation)
                } else {
                    state.next_generation
                },
                job: request_job,
                job_lineage: if same_lineage {
                    previous.map_or(state.next_job_lineage, |owner| owner.job_lineage)
                } else {
                    state.next_job_lineage
                },
            });
            state.cleared = None;
        }
        PickerOwnerPublicationDisposition::Published(publication)
    }

    pub(crate) fn publish_with(
        &self,
        request: PickerOwnerPublicationRequest,
        apply: impl FnOnce(PickerOwnerPublicationRequest) -> PickerOwnerApplyResult,
    ) -> PickerOwnerPublicationDisposition {
        let mut state = self.lock();
        if state.active_leases != 0 {
            if state.deferred.back().copied() != Some(request) {
                state.deferred.push_back(request);
            }
            return PickerOwnerPublicationDisposition::Deferred;
        }
        Self::apply_one(&mut state, request, apply)
    }

    fn begin_lease(&self) {
        let mut state = self.lock();
        state.active_leases = state.active_leases.saturating_add(1);
    }

    fn release_lease_with(
        &self,
        mut apply: impl FnMut(PickerOwnerPublicationRequest) -> PickerOwnerApplyResult,
    ) {
        let mut state = self.lock();
        state.active_leases = state.active_leases.saturating_sub(1);
        if state.active_leases != 0 {
            return;
        }
        while let Some(request) = state.deferred.pop_front() {
            let _ = Self::apply_one(&mut state, request, &mut apply);
        }
    }

    pub(crate) fn register_live_run(
        &self,
        job: usize,
        dialog: usize,
        list: usize,
    ) -> Option<PickerRunRegistration> {
        let mut state = self.lock();
        let owner = state.current?;
        if job == 0
            || dialog == 0
            || list == 0
            || owner.dialog != dialog
            || owner.job != job
            || state.active_leases != 0
            || !state.deferred.is_empty()
        {
            return None;
        }
        state.next_run_lineage = state.next_run_lineage.wrapping_add(1).max(1);
        let run = PickerRunRegistration {
            owner_generation: owner.generation,
            job,
            list,
            job_lineage: owner.job_lineage,
            run_lineage: state.next_run_lineage,
        };
        state.current_run = Some(run);
        Some(run)
    }

    pub(crate) fn token_lineage_is_current(&self, token: PickerProfileRunToken) -> bool {
        let state = self.lock();
        token.job != 0
            && state.current
                == Some(PickerOwnerLineage {
                    dialog: token.dialog,
                    generation: token.owner_generation,
                    job: token.job,
                    job_lineage: token.job_lineage,
                })
            && state.current_run
                == Some(PickerRunRegistration {
                    owner_generation: token.owner_generation,
                    job: token.job,
                    list: token.list,
                    job_lineage: token.job_lineage,
                    run_lineage: token.run_lineage,
                })
    }

    fn begin_live_token_lease(&self, token: PickerProfileRunToken) -> bool {
        let mut state = self.lock();
        let exact = token.job != 0
            && state.current
                == Some(PickerOwnerLineage {
                    dialog: token.dialog,
                    generation: token.owner_generation,
                    job: token.job,
                    job_lineage: token.job_lineage,
                })
            && state.current_run
                == Some(PickerRunRegistration {
                    owner_generation: token.owner_generation,
                    job: token.job,
                    list: token.list,
                    job_lineage: token.job_lineage,
                    run_lineage: token.run_lineage,
                });
        if exact {
            state.active_leases = state.active_leases.saturating_add(1);
        }
        exact
    }

    pub(crate) fn current_live_token(
        &self,
        dialog: usize,
        observed_vtable: usize,
        expected_vtable: usize,
    ) -> Option<PickerProfileRunToken> {
        let state = self.lock();
        let owner = state.current.filter(|owner| owner.dialog == dialog)?;
        let run = state.current_run.filter(|run| {
            run.owner_generation == owner.generation
                && run.job == owner.job
                && run.job_lineage == owner.job_lineage
        })?;
        (owner.job != 0 && observed_vtable == expected_vtable && expected_vtable != 0).then_some(
            PickerProfileRunToken {
                job: owner.job,
                list: run.list,
                dialog,
                owner_generation: owner.generation,
                job_lineage: owner.job_lineage,
                run_lineage: run.run_lineage,
                observed_vtable,
                expected_vtable,
            },
        )
    }

    pub(crate) fn capture_native_removal(
        &self,
        dialog: usize,
        job: usize,
        list: usize,
    ) -> Option<PickerNativeRemovalCapture> {
        if dialog == 0 || job == 0 || list == 0 {
            return None;
        }
        let state = self.lock();
        let owner = state
            .current
            .filter(|owner| owner.dialog == dialog && owner.job == job)?;
        let run = state.current_run.filter(|run| {
            run.owner_generation == owner.generation
                && run.job == owner.job
                && run.list == list
                && run.job_lineage == owner.job_lineage
        })?;
        Some(PickerNativeRemovalCapture { owner, run, list })
    }

    pub(crate) fn cleared_lineage_for_job(&self, job: usize) -> Option<PickerOwnerClearedLineage> {
        let state = self.lock();
        state
            .cleared
            .filter(|lineage| job != 0 && lineage.old_owner.job == job)
    }

    pub(crate) fn cleared_lineage_is_current(&self, lineage: PickerOwnerClearedLineage) -> bool {
        self.lock().cleared == Some(lineage)
    }

    pub(crate) fn native_removal_authority(&self) -> Option<PickerNativeRemovalAuthority> {
        self.lock().native_removal
    }

    pub(crate) fn native_removal_authority_is_current(
        &self,
        authority: PickerNativeRemovalAuthority,
    ) -> bool {
        let state = self.lock();
        state.native_removal == Some(authority)
            && state.current.is_none()
            && state.current_run.is_none()
            && state.cleared == Some(authority.cleared)
    }

    pub(crate) fn commit_native_removal_authority(
        &self,
        authority: PickerNativeRemovalAuthority,
    ) -> bool {
        let mut state = self.lock();
        if state.native_removal != Some(authority) {
            return false;
        }
        state.native_removal = None;
        true
    }

    #[cfg(test)]
    fn snapshot_for_test(
        &self,
    ) -> (
        usize,
        Vec<PickerOwnerPublicationRequest>,
        Option<PickerOwnerLineage>,
        Option<PickerOwnerClearedLineage>,
    ) {
        let state = self.lock();
        (
            state.active_leases,
            state.deferred.iter().copied().collect(),
            state.current,
            state.cleared,
        )
    }
}

static PICKER_OWNER_LIFETIME: std::sync::OnceLock<PickerOwnerLifetimeCoordinator> =
    std::sync::OnceLock::new();
static SAVE_PICKER_LAST_REMOVAL_HANDOFF_ZERO_GENERATION: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn picker_owner_lifetime() -> &'static PickerOwnerLifetimeCoordinator {
    PICKER_OWNER_LIFETIME.get_or_init(PickerOwnerLifetimeCoordinator::default)
}

pub(crate) fn save_picker_native_removal_authority() -> Option<PickerNativeRemovalAuthority> {
    picker_owner_lifetime().native_removal_authority()
}

pub(crate) fn save_picker_native_removal_authority_still_current(
    authority: PickerNativeRemovalAuthority,
) -> bool {
    picker_owner_lifetime().native_removal_authority_is_current(authority)
        && save_picker_pending_resubmit_transition() == Some(authority.pending)
        && save_picker_system_dialog_identity()
            == Some(PickerSystemDialogIdentity {
                dialog: authority.pending.system_dialog,
                generation: authority.pending.system_dialog_generation,
            })
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
        && save_picker_live_profile_dialog() == 0
}

pub(crate) fn commit_save_picker_native_removal_authority(
    authority: PickerNativeRemovalAuthority,
) -> bool {
    picker_owner_lifetime().commit_native_removal_authority(authority)
}

pub(crate) fn picker_native_removal_matches_refresh(
    authority: PickerNativeRemovalAuthority,
    request: PickerRefreshRequest,
) -> bool {
    authority.pending.old_dialog == request.dialog
        && (authority.pending.refresh_owner_generation == request.generation
            || authority.pending.refresh_close_generation == request.generation)
}

pub(crate) fn save_picker_native_removal_owns_refresh(request: PickerRefreshRequest) -> bool {
    let Some(authority) = save_picker_native_removal_authority()
        .filter(|authority| save_picker_native_removal_authority_still_current(*authority))
        .filter(|authority| picker_native_removal_matches_refresh(*authority, request))
    else {
        return false;
    };
    let zero_generation = authority.cleared.zero_generation;
    if SAVE_PICKER_LAST_REMOVAL_HANDOFF_ZERO_GENERATION.swap(zero_generation, Ordering::SeqCst)
        != zero_generation
    {
        er_telemetry::counters::SAVE_PICKER_NATIVE_REMOVAL_TICKET_HANDOFFS
            .fetch_add(1, Ordering::SeqCst);
    } else {
        er_telemetry::counters::SAVE_PICKER_NATIVE_REMOVAL_TICKET_RETRIES
            .fetch_add(1, Ordering::SeqCst);
    }
    er_telemetry::counters::SAVE_PICKER_OWNER_ZERO_LOOP_GUARD_MAX.fetch_max(1, Ordering::SeqCst);
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerNativeRemovalDisposition {
    Published,
    Deferred,
    Stale,
    Foreign,
    RemovalNotProven,
    NoTransition,
}

pub(crate) fn native_menu_window_removal_boundary_with(
    coordinator: &PickerOwnerLifetimeCoordinator,
    captured: Option<PickerNativeRemovalCapture>,
    forward_original: impl FnOnce(),
    removal_proven_after_forward: impl FnOnce(PickerNativeRemovalCapture) -> bool,
    exact_pending_transition: impl FnOnce(
        PickerNativeRemovalCapture,
    ) -> Option<PickerPendingResubmitTransition>,
    apply: impl FnOnce(PickerOwnerPublicationRequest) -> PickerOwnerApplyResult,
) -> PickerNativeRemovalDisposition {
    forward_original();
    let Some(captured) = captured else {
        return PickerNativeRemovalDisposition::Foreign;
    };
    if !removal_proven_after_forward(captured) {
        return PickerNativeRemovalDisposition::RemovalNotProven;
    }
    let Some(pending) = exact_pending_transition(captured) else {
        return PickerNativeRemovalDisposition::NoTransition;
    };
    let disposition = match coordinator.publish_with(
        PickerOwnerPublicationRequest::CompareRemove {
            expected: captured,
            pending,
            new_dialog: 0,
        },
        apply,
    ) {
        PickerOwnerPublicationDisposition::Published(_) => {
            PickerNativeRemovalDisposition::Published
        }
        PickerOwnerPublicationDisposition::Deferred => PickerNativeRemovalDisposition::Deferred,
        PickerOwnerPublicationDisposition::Stale { .. } => PickerNativeRemovalDisposition::Stale,
    };
    disposition
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerSystemDialogIdentity {
    pub(crate) dialog: usize,
    pub(crate) generation: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerSystemDialogPublicationDisposition {
    Published(PickerSystemDialogIdentity),
    Cleared { generation: usize },
    Deferred,
}

#[derive(Debug, Default)]
struct PickerSystemDialogState {
    current: Option<PickerSystemDialogIdentity>,
    next_generation: usize,
    active_leases: usize,
    submit_started: bool,
    deferred: std::collections::VecDeque<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct PickerSystemDialogCoordinator {
    state: std::sync::Mutex<PickerSystemDialogState>,
}

impl PickerSystemDialogCoordinator {
    fn lock(&self) -> std::sync::MutexGuard<'_, PickerSystemDialogState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn apply_one(
        state: &mut PickerSystemDialogState,
        dialog: usize,
        apply: impl FnOnce(usize),
    ) -> PickerSystemDialogPublicationDisposition {
        apply(dialog);
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        if dialog == 0 {
            state.current = None;
            PickerSystemDialogPublicationDisposition::Cleared {
                generation: state.next_generation,
            }
        } else {
            let identity = PickerSystemDialogIdentity {
                dialog,
                generation: state.next_generation,
            };
            state.current = Some(identity);
            PickerSystemDialogPublicationDisposition::Published(identity)
        }
    }

    pub(crate) fn publish_with(
        &self,
        dialog: usize,
        apply: impl FnOnce(usize),
    ) -> PickerSystemDialogPublicationDisposition {
        let mut state = self.lock();
        if state.active_leases != 0 {
            state.deferred.push_back(dialog);
            return PickerSystemDialogPublicationDisposition::Deferred;
        }
        Self::apply_one(&mut state, dialog, apply)
    }

    pub(crate) fn current_identity(&self) -> Option<PickerSystemDialogIdentity> {
        self.lock().current
    }

    pub(crate) fn try_publish_initial_with(
        &self,
        dialog: usize,
        apply: impl FnOnce(usize),
    ) -> Option<PickerSystemDialogIdentity> {
        let mut state = self.lock();
        if dialog == 0
            || state.current.is_some()
            || state.active_leases != 0
            || state.submit_started
            || !state.deferred.is_empty()
        {
            return None;
        }
        apply(dialog);
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        let identity = PickerSystemDialogIdentity {
            dialog,
            generation: state.next_generation,
        };
        state.current = Some(identity);
        Some(identity)
    }

    pub(crate) fn clear_exact_with(
        &self,
        identity: PickerSystemDialogIdentity,
        apply: impl FnOnce(usize),
    ) -> bool {
        let mut state = self.lock();
        if state.current != Some(identity)
            || state.active_leases != 0
            || state.submit_started
            || !state.deferred.is_empty()
        {
            return false;
        }
        apply(0);
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        state.current = None;
        true
    }

    fn begin_lease(&self, identity: PickerSystemDialogIdentity) -> bool {
        let mut state = self.lock();
        if state.current != Some(identity) || state.submit_started || !state.deferred.is_empty() {
            return false;
        }
        state.active_leases = state.active_leases.saturating_add(1);
        true
    }

    fn identity_is_current(&self, identity: PickerSystemDialogIdentity) -> bool {
        self.lock().current == Some(identity)
    }

    /// Linearization point immediately before latch commit/native submit. A publication that won
    /// earlier is already queued and rejects; one that arrives later is ordered after this submit.
    fn begin_submit(&self, identity: PickerSystemDialogIdentity) -> bool {
        let mut state = self.lock();
        if state.active_leases == 0
            || state.current != Some(identity)
            || state.submit_started
            || !state.deferred.is_empty()
        {
            return false;
        }
        state.submit_started = true;
        true
    }

    fn cancel_submit(&self) {
        self.lock().submit_started = false;
    }

    fn release_lease_with(&self, mut apply: impl FnMut(usize)) {
        let mut state = self.lock();
        state.active_leases = state.active_leases.saturating_sub(1);
        if state.active_leases != 0 {
            return;
        }
        state.submit_started = false;
        while let Some(dialog) = state.deferred.pop_front() {
            let _ = Self::apply_one(&mut state, dialog, &mut apply);
        }
    }
}

static PICKER_SYSTEM_DIALOG_COORDINATOR: std::sync::OnceLock<PickerSystemDialogCoordinator> =
    std::sync::OnceLock::new();

fn picker_system_dialog_coordinator() -> &'static PickerSystemDialogCoordinator {
    PICKER_SYSTEM_DIALOG_COORDINATOR.get_or_init(PickerSystemDialogCoordinator::default)
}

fn apply_picker_system_dialog_publication_now(dialog: usize) {
    SAVE_PICKER_SYSTEM_DIALOG.store(dialog, Ordering::SeqCst);
}

pub(crate) fn save_picker_publish_system_dialog(
    dialog: usize,
) -> PickerSystemDialogPublicationDisposition {
    picker_system_dialog_coordinator()
        .publish_with(dialog, apply_picker_system_dialog_publication_now)
}

pub(crate) fn save_picker_system_dialog_identity() -> Option<PickerSystemDialogIdentity> {
    picker_system_dialog_coordinator().current_identity()
}

pub(crate) fn save_picker_try_publish_initial_system_dialog(
    dialog: usize,
) -> Option<PickerSystemDialogIdentity> {
    picker_system_dialog_coordinator()
        .try_publish_initial_with(dialog, apply_picker_system_dialog_publication_now)
}

pub(crate) fn save_picker_clear_exact_system_dialog(identity: PickerSystemDialogIdentity) -> bool {
    picker_system_dialog_coordinator()
        .clear_exact_with(identity, apply_picker_system_dialog_publication_now)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerProfileRunToken {
    pub(crate) job: usize,
    pub(crate) list: usize,
    pub(crate) dialog: usize,
    pub(crate) owner_generation: usize,
    pub(crate) job_lineage: usize,
    pub(crate) run_lineage: usize,
    pub(crate) observed_vtable: usize,
    pub(crate) expected_vtable: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOwnerClearedAuthority {
    pub(crate) observed_job: usize,
    pub(crate) lineage: PickerOwnerClearedLineage,
    pub(crate) pending: PickerPendingResubmitTransition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerProfileRunObservation {
    OtherResource,
    OwnerCleared(PickerOwnerClearedAuthority),
    Live(PickerProfileRunToken),
    Rejected {
        job: usize,
        dialog: usize,
        observed_vtable: usize,
        expected_vtable: usize,
    },
}

impl PickerProfileRunObservation {
    fn live_token(self) -> Option<PickerProfileRunToken> {
        match self {
            Self::Live(token) => Some(token),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerDestinationParentToken {
    pub(crate) job: usize,
    pub(crate) dialog: usize,
    pub(crate) observed_vtable: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerOuterPostAuthority {
    Other,
    DestinationParent(PickerDestinationParentToken),
    NativeRemoval(PickerNativeRemovalAuthority),
    Profile(PickerProfileRunObservation),
}

impl PickerOuterPostAuthority {
    pub(crate) fn allows_destination_open(self) -> bool {
        matches!(self, Self::DestinationParent(_))
    }

    pub(crate) fn live_profile_token(self) -> Option<PickerProfileRunToken> {
        match self {
            Self::Profile(observation) => observation.live_token(),
            _ => None,
        }
    }

    pub(crate) fn allows_owner_zero_resubmit(self) -> bool {
        matches!(
            self,
            Self::NativeRemoval(_) | Self::Profile(PickerProfileRunObservation::OwnerCleared(_))
        )
    }

    pub(crate) fn allows_initial_parent_submit(self) -> bool {
        matches!(self, Self::DestinationParent(_))
    }
}

pub(crate) fn observe_picker_destination_parent_with(
    exact_parent_resource: bool,
    job: usize,
    dialog: usize,
    observed_vtable: usize,
    published_dialog: usize,
) -> Option<PickerDestinationParentToken> {
    (exact_parent_resource
        && job != 0
        && dialog != 0
        && observed_vtable != 0
        && dialog == published_dialog)
        .then_some(PickerDestinationParentToken {
            job,
            dialog,
            observed_vtable,
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOuterPostPermissions {
    pub(crate) destination_open: bool,
    pub(crate) live_profile_token: Option<PickerProfileRunToken>,
    pub(crate) picker_submit: bool,
}

impl PickerOuterPostPermissions {
    pub(crate) fn run_destination<R>(self, operation: impl FnOnce() -> R) -> Option<R> {
        self.destination_open.then(operation)
    }

    pub(crate) fn run_live_profile<R>(
        self,
        operation: impl FnOnce(PickerProfileRunToken) -> R,
    ) -> Option<R> {
        self.live_profile_token.map(operation)
    }

    pub(crate) fn run_picker_submit<R>(self, operation: impl FnOnce() -> R) -> Option<R> {
        self.picker_submit.then(operation)
    }
}

fn picker_destination_parent_token_still_current_with(
    token: PickerDestinationParentToken,
    published_dialog: usize,
    read_vtable: impl FnOnce(usize) -> Option<usize>,
) -> bool {
    token.dialog != 0
        && token.dialog == published_dialog
        && read_vtable(token.dialog) == Some(token.observed_vtable)
}

fn picker_owner_cleared_authority_matches(
    authority: PickerOwnerClearedAuthority,
    current_lineage: Option<PickerOwnerClearedLineage>,
    current_pending: Option<PickerPendingResubmitTransition>,
) -> bool {
    authority.observed_job != 0
        && authority.observed_job == authority.lineage.old_owner.job
        && current_lineage == Some(authority.lineage)
        && current_pending == Some(authority.pending)
}

fn picker_owner_cleared_authority_still_current(authority: PickerOwnerClearedAuthority) -> bool {
    picker_owner_cleared_authority_matches(
        authority,
        picker_owner_lifetime().cleared_lineage_for_job(authority.observed_job),
        save_picker_pending_resubmit_transition()
            .filter(|_| SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0),
    )
}

pub(crate) fn picker_outer_authority_still_current_with(
    authority: PickerOuterPostAuthority,
    load_profile_owner: impl FnOnce() -> usize,
    load_parent_owner: impl FnOnce() -> usize,
    token_lineage_is_current: impl FnOnce(PickerProfileRunToken) -> bool,
    native_removal_is_current: impl FnOnce(PickerNativeRemovalAuthority) -> bool,
    read_vtable: impl FnMut(usize) -> Option<usize>,
) -> bool {
    match authority {
        PickerOuterPostAuthority::Other => false,
        PickerOuterPostAuthority::DestinationParent(token) => {
            picker_destination_parent_token_still_current_with(
                token,
                load_parent_owner(),
                read_vtable,
            )
        }
        PickerOuterPostAuthority::NativeRemoval(authority) => {
            load_profile_owner() == 0 && native_removal_is_current(authority)
        }
        PickerOuterPostAuthority::Profile(PickerProfileRunObservation::OwnerCleared(authority)) => {
            load_profile_owner() == 0 && picker_owner_cleared_authority_still_current(authority)
        }
        PickerOuterPostAuthority::Profile(PickerProfileRunObservation::Live(token)) => {
            picker_profile_token_still_current_with(
                token,
                load_profile_owner(),
                token_lineage_is_current,
                read_vtable,
            )
        }
        PickerOuterPostAuthority::Profile(PickerProfileRunObservation::OtherResource)
        | PickerOuterPostAuthority::Profile(PickerProfileRunObservation::Rejected { .. }) => false,
    }
}

pub(crate) fn picker_outer_authority_still_current(authority: PickerOuterPostAuthority) -> bool {
    picker_outer_authority_still_current_with(
        authority,
        save_picker_live_profile_dialog,
        || SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst),
        |token| picker_owner_lifetime().token_lineage_is_current(token),
        save_picker_native_removal_authority_still_current,
        |dialog| unsafe { safe_read_usize(dialog) },
    )
}

pub(crate) fn picker_outer_post_permissions_with(
    authority: PickerOuterPostAuthority,
    destination_open_pending: bool,
    initial_open_pending: bool,
    reopen_pending: bool,
) -> PickerOuterPostPermissions {
    PickerOuterPostPermissions {
        destination_open: authority.allows_destination_open() && destination_open_pending,
        live_profile_token: authority.live_profile_token(),
        picker_submit: (authority.allows_initial_parent_submit()
            && initial_open_pending
            && !reopen_pending)
            || (authority.allows_owner_zero_resubmit() && (initial_open_pending || reopen_pending)),
    }
}

fn observe_picker_profile_run_with(
    resource_is_profile_select: bool,
    job: usize,
    observed_list: usize,
    observed_dialog: usize,
    observed_vtable: usize,
    published_dialog: usize,
    expected_vtable: usize,
    run: Option<PickerRunRegistration>,
) -> PickerProfileRunObservation {
    if !resource_is_profile_select {
        return PickerProfileRunObservation::OtherResource;
    }
    if observed_dialog == 0 {
        return PickerProfileRunObservation::OtherResource;
    }
    if job != 0
        && observed_list != 0
        && observed_dialog == published_dialog
        && expected_vtable != 0
        && observed_vtable == expected_vtable
        && run.is_some_and(|run| {
            run.job == job
                && run.list == observed_list
                && run.owner_generation != 0
                && run.job_lineage != 0
                && run.run_lineage != 0
        })
    {
        let run = run.expect("checked exact Run registration");
        return PickerProfileRunObservation::Live(PickerProfileRunToken {
            job,
            list: observed_list,
            dialog: observed_dialog,
            owner_generation: run.owner_generation,
            job_lineage: run.job_lineage,
            run_lineage: run.run_lineage,
            observed_vtable,
            expected_vtable,
        });
    }
    PickerProfileRunObservation::Rejected {
        job,
        dialog: observed_dialog,
        observed_vtable,
        expected_vtable,
    }
}

pub(crate) fn save_picker_observe_profile_select_run(
    resource_is_profile_select: bool,
    job: usize,
    observed_dialog: usize,
    observed_vtable: usize,
) -> PickerProfileRunObservation {
    let published_dialog = save_picker_live_profile_dialog();
    let observed_list =
        unsafe { safe_read_usize(job + MENU_WINDOW_JOB_PUSH_TARGET_50_OFFSET) }.unwrap_or(0);
    let expected_vtable = game_module_base()
        .ok()
        .and_then(|base| base.checked_add(PROFILE_LOAD_DIALOG_VTABLE_RVA))
        .unwrap_or(0);
    let run = (resource_is_profile_select
        && job != 0
        && observed_dialog != 0
        && observed_dialog == published_dialog
        && expected_vtable != 0
        && observed_vtable == expected_vtable)
        .then(|| picker_owner_lifetime().register_live_run(job, observed_dialog, observed_list))
        .flatten();
    let observation = if resource_is_profile_select && observed_dialog == 0 {
        picker_owner_lifetime()
            .cleared_lineage_for_job(job)
            .and_then(|lineage| {
                save_picker_pending_resubmit_transition().map(|pending| {
                    PickerProfileRunObservation::OwnerCleared(PickerOwnerClearedAuthority {
                        observed_job: job,
                        lineage,
                        pending,
                    })
                })
            })
            .unwrap_or(PickerProfileRunObservation::Rejected {
                job,
                dialog: 0,
                observed_vtable,
                expected_vtable,
            })
    } else {
        observe_picker_profile_run_with(
            resource_is_profile_select,
            job,
            observed_list,
            observed_dialog,
            observed_vtable,
            published_dialog,
            expected_vtable,
            run,
        )
    };
    let source = match observation {
        PickerProfileRunObservation::OtherResource => 0,
        PickerProfileRunObservation::Live(_) => 1,
        PickerProfileRunObservation::OwnerCleared(_) => 2,
        PickerProfileRunObservation::Rejected { .. } => 3,
    };
    er_telemetry::counters::SAVE_PICKER_PROFILE_RUN_SOURCE_LAST.store(source, Ordering::SeqCst);
    if !matches!(observation, PickerProfileRunObservation::OtherResource) {
        er_telemetry::counters::SAVE_PICKER_PROFILE_RUN_LAST_JOB.store(job, Ordering::SeqCst);
        er_telemetry::counters::SAVE_PICKER_PROFILE_RUN_LAST_DIALOG
            .store(observed_dialog, Ordering::SeqCst);
        er_telemetry::counters::SAVE_PICKER_PROFILE_RUN_LAST_VTABLE
            .store(observed_vtable, Ordering::SeqCst);
        er_telemetry::counters::SAVE_PICKER_PROFILE_RUN_EXPECTED_VTABLE
            .store(expected_vtable, Ordering::SeqCst);
    }
    match observation {
        PickerProfileRunObservation::Live(_) => {
            er_telemetry::counters::SAVE_PICKER_PROFILE_RUN_TOKEN_ACCEPTS
                .fetch_add(1, Ordering::SeqCst);
        }
        PickerProfileRunObservation::Rejected { .. } => {
            let rejects = er_telemetry::counters::SAVE_PICKER_PROFILE_RUN_TOKEN_REJECTIONS
                .fetch_add(1, Ordering::SeqCst)
                + 1;
            if rejects <= 8 || rejects.is_power_of_two() {
                append_autoload_debug(format_args!(
                    "save-picker: rejected 05_010 Run owner token job=0x{job:x} observed=0x{observed_dialog:x} published=0x{published_dialog:x} vtable=0x{observed_vtable:x} expected=0x{expected_vtable:x} rejects={rejects}"
                ));
            }
        }
        _ => {}
    }
    observation
}

pub(crate) fn picker_profile_token_still_current_with(
    token: PickerProfileRunToken,
    published_dialog: usize,
    token_lineage_is_current: impl FnOnce(PickerProfileRunToken) -> bool,
    mut read_vtable: impl FnMut(usize) -> Option<usize>,
) -> bool {
    token.job != 0
        && token.owner_generation != 0
        && token.job_lineage != 0
        && token.run_lineage != 0
        && token_lineage_is_current(token)
        && token.dialog != 0
        && token.dialog == published_dialog
        && token.expected_vtable != 0
        && token.observed_vtable == token.expected_vtable
        && read_vtable(token.dialog) == Some(token.expected_vtable)
}

pub(crate) fn save_picker_profile_token_still_current(token: PickerProfileRunToken) -> bool {
    picker_profile_token_still_current_with(
        token,
        save_picker_live_profile_dialog(),
        |token| picker_owner_lifetime().token_lineage_is_current(token),
        |dialog| unsafe { safe_read_usize(dialog) },
    )
}

fn picker_deferred_close_token_allows(
    observation: PickerProfileRunObservation,
    dialog: usize,
) -> bool {
    dialog != 0
        && observation
            .live_token()
            .is_some_and(|token| token.dialog == dialog)
}

pub(crate) fn pump_picker_native_maintenance_with(
    observation: PickerProfileRunObservation,
    mut pump_path_editor_pointer_free: impl FnMut(),
    mut pump_path_editor_native_submit: impl FnMut(PickerProfileRunToken),
    mut pump_drive: impl FnMut(PickerProfileRunToken),
    mut pump_scrollbar: impl FnMut(PickerProfileRunToken),
    mut pump_edge: impl FnMut(PickerProfileRunToken),
) {
    pump_path_editor_pointer_free();
    if let Some(token) = observation.live_token() {
        pump_path_editor_native_submit(token);
        pump_drive(token);
        pump_scrollbar(token);
        pump_edge(token);
    }
}

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerNativeCloseRejectReason {
    MissingDialog = 1,
    PublishedOwnerMismatch = 2,
    MissingExpectedVtable = 3,
    UnreadableVtable = 4,
    UnexpectedVtable = 5,
    InvalidTokenLineage = 6,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerNativeCloseInvokeOutcome {
    Closed,
    PreflightRejected(PickerNativeCloseRejectReason),
    ResolveFailed,
}

impl PickerNativeCloseInvokeOutcome {
    fn is_closed(self) -> bool {
        self == Self::Closed
    }
}

fn picker_native_close_preflight_with(
    dialog: usize,
    current_dialog: usize,
    expected_vtable: usize,
    mut read_vtable: impl FnMut(usize) -> Option<usize>,
) -> Result<usize, PickerNativeCloseRejectReason> {
    if dialog == 0 {
        return Err(PickerNativeCloseRejectReason::MissingDialog);
    }
    if dialog != current_dialog {
        return Err(PickerNativeCloseRejectReason::PublishedOwnerMismatch);
    }
    if expected_vtable == 0 {
        return Err(PickerNativeCloseRejectReason::MissingExpectedVtable);
    }
    let Some(observed_vtable) = read_vtable(dialog) else {
        return Err(PickerNativeCloseRejectReason::UnreadableVtable);
    };
    if observed_vtable != expected_vtable {
        return Err(PickerNativeCloseRejectReason::UnexpectedVtable);
    }
    Ok(observed_vtable)
}

fn execute_picker_live_token_call_on_coordinator_with<R>(
    coordinator: &PickerOwnerLifetimeCoordinator,
    token: PickerProfileRunToken,
    mut load_published_owner: impl FnMut() -> usize,
    read_vtable: impl FnMut(usize) -> Option<usize>,
    native_call: impl FnOnce(usize) -> R,
    apply_deferred: impl FnMut(PickerOwnerPublicationRequest) -> PickerOwnerApplyResult,
) -> Result<(usize, usize, R), PickerNativeCloseRejectReason> {
    // Mark the owner lifetime leased before either validation callback. Publication never waits on
    // this function and no mutex spans callbacks/native code: synchronous re-entry records a
    // deferred publication, which is applied exactly once after the sink returns (or validation
    // rejects). Thus valid owner/vtable evidence cannot be retired between resolution and call.
    if !coordinator.begin_live_token_lease(token) {
        return Err(PickerNativeCloseRejectReason::InvalidTokenLineage);
    }
    let current_dialog = load_published_owner();
    let preflight = picker_native_close_preflight_with(
        token.dialog,
        current_dialog,
        token.expected_vtable,
        read_vtable,
    );
    let result = match preflight {
        Ok(observed_vtable) => {
            let value = native_call(token.dialog);
            Ok((current_dialog, observed_vtable, value))
        }
        Err(reason) => Err(reason),
    };
    coordinator.release_lease_with(apply_deferred);
    result
}

pub(crate) fn execute_picker_live_token_call_with<R>(
    token: PickerProfileRunToken,
    load_published_owner: impl FnMut() -> usize,
    read_vtable: impl FnMut(usize) -> Option<usize>,
    native_call: impl FnOnce(usize) -> R,
) -> Result<(usize, usize, R), PickerNativeCloseRejectReason> {
    execute_picker_live_token_call_on_coordinator_with(
        picker_owner_lifetime(),
        token,
        load_published_owner,
        read_vtable,
        native_call,
        crate::save_picker_path_editor::apply_picker_owner_publication_now,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerResubmitDisposition {
    WaitingForOwnerClear,
    StageFailed,
    AuthorizationLost,
    Submitted { opened: bool },
}

pub(crate) fn apply_picker_resubmit_model_lifetime_with(
    disposition: PickerResubmitDisposition,
    reopen_as_picker: bool,
    clear_model_and_mode: impl FnOnce(),
) {
    if reopen_as_picker
        && matches!(
            disposition,
            PickerResubmitDisposition::Submitted { opened: true }
        )
    {
        clear_model_and_mode();
    }
}

#[cfg(test)]
pub(crate) fn execute_picker_resubmit_with(
    old_owner: usize,
    final_authorized: impl FnOnce() -> bool,
    before_submit: impl FnOnce(),
    submit: impl FnOnce() -> bool,
) -> PickerResubmitDisposition {
    if old_owner != 0 {
        return PickerResubmitDisposition::WaitingForOwnerClear;
    }
    if !final_authorized() {
        return PickerResubmitDisposition::AuthorizationLost;
    }
    before_submit();
    PickerResubmitDisposition::Submitted { opened: submit() }
}

pub(crate) fn execute_owner_zero_resubmit_transaction_on_coordinator_with<T: Copy, C>(
    coordinator: &PickerOwnerLifetimeCoordinator,
    mut validate_authority: impl FnMut() -> bool,
    mut reserve_transition: impl FnMut() -> Option<T>,
    mut stage_latest_model: impl FnMut() -> bool,
    mut begin_submit: impl FnMut() -> bool,
    mut cancel_submit: impl FnMut(),
    mut commit_transition: impl FnMut(T) -> C,
    mut release_transition: impl FnMut(T),
    mut rollback_stage: impl FnMut(),
    mut commit_stage: impl FnMut(),
    mut native_submit: impl FnMut() -> bool,
    apply_deferred: impl FnMut(PickerOwnerPublicationRequest) -> PickerOwnerApplyResult,
) -> PickerResubmitDisposition {
    coordinator.begin_lease();
    let disposition = if !validate_authority() {
        PickerResubmitDisposition::AuthorizationLost
    } else if let Some(reservation) = reserve_transition() {
        if !stage_latest_model() {
            rollback_stage();
            release_transition(reservation);
            PickerResubmitDisposition::StageFailed
        } else if !validate_authority() || !begin_submit() {
            rollback_stage();
            release_transition(reservation);
            PickerResubmitDisposition::AuthorizationLost
        } else if native_submit() {
            // Reservation excludes every transition writer, while owner/System leases defer
            // lifetime publication. Successful native submit therefore makes exact latch and
            // presentation commit an infallible consequence of the still-owned reservation.
            commit_transition(reservation);
            commit_stage();
            PickerResubmitDisposition::Submitted { opened: true }
        } else {
            cancel_submit();
            rollback_stage();
            release_transition(reservation);
            PickerResubmitDisposition::Submitted { opened: false }
        }
    } else {
        PickerResubmitDisposition::AuthorizationLost
    };
    coordinator.release_lease_with(apply_deferred);
    disposition
}

#[cfg(test)]
fn execute_owner_zero_submit_on_coordinator_with<R>(
    coordinator: &PickerOwnerLifetimeCoordinator,
    validate: impl FnOnce() -> bool,
    claim_transition: impl FnOnce() -> bool,
    native_submit: impl FnOnce() -> R,
    apply_deferred: impl FnMut(PickerOwnerPublicationRequest) -> PickerOwnerApplyResult,
) -> Option<R> {
    coordinator.begin_lease();
    let result = (validate() && claim_transition()).then(native_submit);
    coordinator.release_lease_with(apply_deferred);
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOwnerClearedResubmitAuthority {
    pub(crate) picker: PickerOwnerClearedAuthority,
    pub(crate) system: PickerSystemDialogIdentity,
}

pub(crate) fn commit_picker_native_removal_after_resubmit(
    authority: PickerOuterPostAuthority,
    disposition: PickerResubmitDisposition,
) {
    if matches!(
        disposition,
        PickerResubmitDisposition::Submitted { opened: true }
    ) && let PickerOuterPostAuthority::NativeRemoval(removal) = authority
        && commit_save_picker_native_removal_authority(removal)
    {
        er_telemetry::counters::SAVE_PICKER_NATIVE_REMOVAL_TICKET_COMMITS
            .fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) fn execute_picker_native_removal_resubmit(
    authority: PickerNativeRemovalAuthority,
    stage_latest_model: impl FnMut() -> bool,
    rollback_stage: impl FnMut(),
    commit_stage: impl FnMut(),
    native_submit: impl FnMut() -> bool,
) -> PickerResubmitDisposition {
    execute_picker_owner_cleared_resubmit(
        PickerOwnerClearedResubmitAuthority {
            picker: PickerOwnerClearedAuthority {
                observed_job: authority.cleared.old_owner.job,
                lineage: authority.cleared,
                pending: authority.pending,
            },
            system: PickerSystemDialogIdentity {
                dialog: authority.pending.system_dialog,
                generation: authority.pending.system_dialog_generation,
            },
        },
        stage_latest_model,
        rollback_stage,
        commit_stage,
        native_submit,
    )
}

pub(crate) fn execute_picker_owner_cleared_resubmit(
    authority: PickerOwnerClearedResubmitAuthority,
    stage_latest_model: impl FnMut() -> bool,
    rollback_stage: impl FnMut(),
    commit_stage: impl FnMut(),
    native_submit: impl FnMut() -> bool,
) -> PickerResubmitDisposition {
    let system_coordinator = picker_system_dialog_coordinator();
    if !system_coordinator.begin_lease(authority.system) {
        return PickerResubmitDisposition::AuthorizationLost;
    }
    let disposition = execute_owner_zero_resubmit_transaction_on_coordinator_with(
        picker_owner_lifetime(),
        || {
            system_coordinator.identity_is_current(authority.system)
                && SAVE_PICKER_SYSTEM_DIALOG.load(Ordering::SeqCst) == authority.system.dialog
                && save_picker_live_profile_dialog() == 0
                && picker_owner_cleared_authority_still_current(authority.picker)
        },
        || reserve_picker_pending_resubmit_transition(authority.picker.pending),
        stage_latest_model,
        || system_coordinator.begin_submit(authority.system),
        || system_coordinator.cancel_submit(),
        commit_picker_pending_resubmit_reservation,
        |reservation| {
            let _ = release_picker_pending_resubmit_reservation(reservation);
        },
        rollback_stage,
        commit_stage,
        native_submit,
        crate::save_picker_path_editor::apply_picker_owner_publication_now,
    );
    system_coordinator.release_lease_with(apply_picker_system_dialog_publication_now);
    save_picker_apply_deferred_resubmit_reset();
    disposition
}

fn execute_picker_destination_resubmit_on_coordinator_with<T: Copy>(
    coordinator: &PickerSystemDialogCoordinator,
    old_owner: usize,
    system_identity: PickerSystemDialogIdentity,
    mut reserve_transition: impl FnMut() -> Option<T>,
    mut release_transition: impl FnMut(T),
    mut commit_transition: impl FnMut(T),
    final_authorized: impl FnOnce() -> bool,
    before_submit: impl FnOnce(),
    submit: impl FnOnce() -> bool,
    apply_deferred: impl FnMut(usize),
) -> PickerResubmitDisposition {
    if !coordinator.begin_lease(system_identity) {
        return PickerResubmitDisposition::AuthorizationLost;
    }
    let disposition = if old_owner != 0 {
        PickerResubmitDisposition::WaitingForOwnerClear
    } else if let Some(reservation) = reserve_transition() {
        if !final_authorized() || !coordinator.begin_submit(system_identity) {
            release_transition(reservation);
            PickerResubmitDisposition::AuthorizationLost
        } else {
            before_submit();
            if submit() {
                commit_transition(reservation);
                PickerResubmitDisposition::Submitted { opened: true }
            } else {
                coordinator.cancel_submit();
                release_transition(reservation);
                PickerResubmitDisposition::Submitted { opened: false }
            }
        }
    } else {
        PickerResubmitDisposition::AuthorizationLost
    };
    coordinator.release_lease_with(apply_deferred);
    disposition
}

pub(crate) fn execute_picker_destination_resubmit(
    old_owner: usize,
    system_identity: PickerSystemDialogIdentity,
    final_authorized: impl FnOnce() -> bool,
    before_submit: impl FnOnce(),
    submit: impl FnOnce() -> bool,
) -> PickerResubmitDisposition {
    let disposition = execute_picker_destination_resubmit_on_coordinator_with(
        picker_system_dialog_coordinator(),
        old_owner,
        system_identity,
        || reserve_picker_destination_resubmit_transition(system_identity),
        |reservation| {
            let _ = release_picker_destination_resubmit_reservation(reservation);
        },
        commit_picker_destination_resubmit_reservation,
        final_authorized,
        before_submit,
        submit,
        apply_picker_system_dialog_publication_now,
    );
    save_picker_apply_deferred_resubmit_reset();
    disposition
}

fn execute_picker_native_close_with(
    token: PickerProfileRunToken,
    load_published_owner: impl FnMut() -> usize,
    read_vtable: impl FnMut(usize) -> Option<usize>,
    close: impl FnOnce(usize),
) -> Result<(usize, usize, ()), PickerNativeCloseRejectReason> {
    execute_picker_live_token_call_with(token, load_published_owner, read_vtable, close)
}

fn picker_native_close_source(reason: &str) -> usize {
    match reason {
        "fresh-owner-refresh" => 1,
        "picked-file" => 2,
        "deferred-path-editor-lease" => 3,
        "new-file" => 4,
        _ => 5,
    }
}

fn save_picker_current_live_token(dialog: usize) -> Option<PickerProfileRunToken> {
    let expected_vtable = game_module_base()
        .ok()
        .and_then(|base| base.checked_add(PROFILE_LOAD_DIALOG_VTABLE_RVA))?;
    let observed_vtable = unsafe { safe_read_usize(dialog) }?;
    picker_owner_lifetime().current_live_token(dialog, observed_vtable, expected_vtable)
}

unsafe fn save_picker_invoke_native_close(
    token: PickerProfileRunToken,
    reason: &str,
) -> PickerNativeCloseInvokeOutcome {
    let dialog = token.dialog;
    let expected_vtable = token.expected_vtable;
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_DIALOG.store(dialog, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_CURRENT.store(0, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_EXPECTED_VTABLE
        .store(expected_vtable, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_SOURCE
        .store(picker_native_close_source(reason), Ordering::SeqCst);

    let Ok(close_addr) = game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) else {
        append_autoload_debug(format_args!(
            "save-picker: FAILED to resolve native close rva for dialog=0x{dialog:x} reason={reason}"
        ));
        return PickerNativeCloseInvokeOutcome::ResolveFailed;
    };
    let close_fn: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(close_addr) };
    let final_current = std::cell::Cell::new(0usize);
    let observed = std::cell::Cell::new(0usize);
    let preflight = execute_picker_native_close_with(
        token,
        || {
            let current = save_picker_live_profile_dialog();
            final_current.set(current);
            current
        },
        |owner| {
            let value = unsafe { safe_read_usize(owner) };
            observed.set(value.unwrap_or(0));
            value
        },
        |owner| unsafe { close_fn(owner) },
    );
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_CURRENT
        .store(final_current.get(), Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_VTABLE
        .store(observed.get(), Ordering::SeqCst);
    if let Err(rejection) = preflight {
        let rejects = er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_REJECTIONS
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_REJECT_REASON
            .store(rejection as usize, Ordering::SeqCst);
        if rejects <= 8 || rejects.is_power_of_two() {
            append_autoload_debug(format_args!(
                "save-picker: native close REJECTED fail-closed reason={rejection:?} dialog=0x{dialog:x} current=0x{:x} vtable=0x{:x} expected=0x{expected_vtable:x} source={reason} rejects={rejects}",
                final_current.get(),
                observed.get()
            ));
        }
        return PickerNativeCloseInvokeOutcome::PreflightRejected(rejection);
    }
    er_telemetry::counters::SAVE_PICKER_CLOSE_PREFLIGHT_LAST_REJECT_REASON
        .store(0, Ordering::SeqCst);

    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
    if SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0 {
        er_telemetry::counters::SAVE_PICKER_REFRESH_NATIVE_CLOSES.fetch_add(1, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "save-picker: native-closed picker window dialog=0x{dialog:x} reason={reason} preflight_vtable=0x{:x}",
        observed.get()
    ));
    PickerNativeCloseInvokeOutcome::Closed
}

/// Native cancel-close (SetResult(Failed) + window close). The coordinator owns generation/reset
/// ordering; the central sink independently revalidates the published owner and exact ProfileLoad
/// vtable immediately before the native virtual call.
pub(crate) unsafe fn save_picker_native_close(dialog: usize, reason: &str) -> bool {
    let Some(token) = save_picker_current_live_token(dialog) else {
        return false;
    };
    match save_picker_path_editor_close_with(dialog, |owned_dialog| {
        owned_dialog == token.dialog
            && unsafe { save_picker_invoke_native_close(token, reason).is_closed() }
    }) {
        er_save_picker::PathEditorCloseDisposition::Closed {
            closed,
            invalidated,
        } => {
            if invalidated {
                SAVE_PICKER_PATH_EDITOR_SUBMIT_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            }
            closed
        }
        er_save_picker::PathEditorCloseDisposition::Deferred(ticket) => {
            SAVE_PICKER_PATH_EDITOR_RESET_DEFERRED.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker: native close deferred dialog=0x{:x} generation={} exact_owner=0x{:x} reason={reason}",
                ticket.dialog, ticket.generation, ticket.owner.current_dialog
            ));
            false
        }
        er_save_picker::PathEditorCloseDisposition::ResetInProgress => {
            SAVE_PICKER_PATH_EDITOR_RESET_DEFERRED.fetch_add(1, Ordering::SeqCst);
            false
        }
        er_save_picker::PathEditorCloseDisposition::Cancelled(ticket) => {
            SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_CANCELS.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker: native close cancelled without dereference dialog=0x{dialog:x} owned_ticket={ticket:?} reason={reason}"
            ));
            false
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerRefreshCloseDisposition {
    Closed,
    Deferred(er_save_picker::PathEditorDeferredCloseTicket),
    ResetInProgress,
    Rejected,
    PreflightRejected,
    Cancelled(Option<er_save_picker::PathEditorDeferredCloseTicket>),
    ResolveFailed,
}

unsafe fn save_picker_refresh_native_close(
    token: PickerProfileRunToken,
    reason: &str,
) -> PickerRefreshCloseDisposition {
    if !save_picker_profile_token_still_current(token)
        || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0
    {
        return PickerRefreshCloseDisposition::PreflightRejected;
    }
    let invoke_outcome = std::cell::Cell::new(PickerNativeCloseInvokeOutcome::ResolveFailed);
    match save_picker_path_editor_close_with(token.dialog, |owned_dialog| {
        let outcome = if owned_dialog == token.dialog {
            unsafe { save_picker_invoke_native_close(token, reason) }
        } else {
            PickerNativeCloseInvokeOutcome::PreflightRejected(
                PickerNativeCloseRejectReason::PublishedOwnerMismatch,
            )
        };
        invoke_outcome.set(outcome);
        outcome.is_closed()
    }) {
        er_save_picker::PathEditorCloseDisposition::Closed {
            closed,
            invalidated,
        } => {
            if invalidated {
                SAVE_PICKER_PATH_EDITOR_SUBMIT_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            }
            if closed {
                PickerRefreshCloseDisposition::Closed
            } else {
                match invoke_outcome.get() {
                    PickerNativeCloseInvokeOutcome::PreflightRejected(_) => {
                        PickerRefreshCloseDisposition::PreflightRejected
                    }
                    PickerNativeCloseInvokeOutcome::ResolveFailed => {
                        PickerRefreshCloseDisposition::ResolveFailed
                    }
                    PickerNativeCloseInvokeOutcome::Closed => {
                        PickerRefreshCloseDisposition::Rejected
                    }
                }
            }
        }
        er_save_picker::PathEditorCloseDisposition::Deferred(ticket) => {
            SAVE_PICKER_PATH_EDITOR_RESET_DEFERRED.fetch_add(1, Ordering::SeqCst);
            PickerRefreshCloseDisposition::Deferred(ticket)
        }
        er_save_picker::PathEditorCloseDisposition::ResetInProgress => {
            SAVE_PICKER_PATH_EDITOR_RESET_DEFERRED.fetch_add(1, Ordering::SeqCst);
            PickerRefreshCloseDisposition::ResetInProgress
        }
        er_save_picker::PathEditorCloseDisposition::Cancelled(ticket) => {
            SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_CANCELS.fetch_add(1, Ordering::SeqCst);
            PickerRefreshCloseDisposition::Cancelled(ticket)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerOwnerListPresence {
    Ambiguous,
    Present,
    Absent,
}

pub(crate) fn picker_owner_list_presence_with(
    profile: usize,
    count: Option<usize>,
    mut read_slot: impl FnMut(usize) -> Option<usize>,
) -> PickerOwnerListPresence {
    if profile == 0 {
        return PickerOwnerListPresence::Ambiguous;
    }
    let Some(count) = count.filter(|count| *count <= 8) else {
        return PickerOwnerListPresence::Ambiguous;
    };
    for index in 0..count {
        let Some(owner) = read_slot(index) else {
            return PickerOwnerListPresence::Ambiguous;
        };
        if owner == profile {
            return PickerOwnerListPresence::Present;
        }
    }
    PickerOwnerListPresence::Absent
}

pub(crate) fn save_picker_owner_transition_pending_for(profile: usize) -> bool {
    if profile == 0 {
        return false;
    }
    if SAVE_PICKER_OPEN_SLOTS_PENDING.load(Ordering::SeqCst) != 0 {
        return true;
    }
    if load_picker_refresh_request().is_some_and(|request| request.dialog == profile) {
        return true;
    }
    if load_path_editor_return_reopen_request().is_some_and(|request| request.dialog == profile) {
        return true;
    }
    SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0
        && er_telemetry::counters::SAVE_PICKER_REFRESH_LAST_OLD_OWNER.load(Ordering::SeqCst)
            == profile
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerAbsentOwnerPublication {
    Published,
    Ambiguous,
    Stale,
}

fn publish_absent_picker_owner_with(
    profile: usize,
    current: usize,
    transition_pending: bool,
    mut publish_zero: impl FnMut() -> usize,
) -> PickerAbsentOwnerPublication {
    if profile == 0 || profile != current {
        return PickerAbsentOwnerPublication::Stale;
    }
    if !transition_pending {
        return PickerAbsentOwnerPublication::Ambiguous;
    }
    if publish_zero() == profile {
        PickerAbsentOwnerPublication::Published
    } else {
        PickerAbsentOwnerPublication::Stale
    }
}

pub(crate) fn save_picker_publish_absent_profile_owner(profile: usize) -> bool {
    let current = save_picker_live_profile_dialog();
    let transition_pending = save_picker_owner_transition_pending_for(profile);
    let refresh_request = load_picker_refresh_request().filter(|request| request.dialog == profile);
    if let Some(request) = refresh_request {
        let _latch_guard = resubmit_latch_lock();
        if !any_resubmit_reserved() {
            SAVE_PICKER_REOPEN_PENDING.store(1, Ordering::SeqCst);
            let _ = arm_picker_pending_resubmit_transition(request.dialog, 0, request.generation);
        }
    }
    let disposition =
        publish_absent_picker_owner_with(profile, current, transition_pending, || {
            match save_picker_path_editor_publish_owner_if_current(profile, 0) {
                PickerOwnerPublicationDisposition::Published(publication) => {
                    if publication.cancelled_close.is_some() {
                        SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_CANCELS
                            .fetch_add(1, Ordering::SeqCst);
                    }
                    publication.previous
                }
                PickerOwnerPublicationDisposition::Stale { actual } => actual,
                PickerOwnerPublicationDisposition::Deferred => 0,
            }
        });
    if disposition == PickerAbsentOwnerPublication::Published {
        er_telemetry::counters::SAVE_PICKER_OWNER_ABSENT_PUBLICATIONS
            .fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: exact owning-list absence published ProfileSelect owner zero without dereference old_owner=0x{profile:x}; pending reopen preserved"
        ));
        true
    } else {
        false
    }
}
