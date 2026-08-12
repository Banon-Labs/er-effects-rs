use std::collections::VecDeque;
use std::sync::Mutex;

const RETIRED_JOB_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathEditorPickerIdentity {
    pub picker_mode_active: bool,
    pub current_dialog: usize,
}

impl PathEditorPickerIdentity {
    pub fn owns(self, dialog: usize) -> bool {
        self.picker_mode_active && dialog != 0 && self.current_dialog == dialog
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathEditorRequestTicket {
    pub dialog: usize,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathEditorActiveProvenance {
    pub generation: u64,
    pub job: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PathEditorActiveTicket {
    request: PathEditorRequestTicket,
    job: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEditorLifecycleRejection {
    InvalidDialog,
    IdentityMismatch,
    Busy,
    RequestMismatch,
    InvalidJob,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEditorResultOwnership {
    Current,
    StaleOwned,
    Foreign,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEditorResetLease {
    Acquired { invalidated: bool },
    DeferredForSubmit,
    DeferredForReset,
}

#[derive(Debug)]
pub struct PathEditorLifecycle<T> {
    generation: u64,
    owner_dialog: usize,
    pending: Option<PathEditorRequestTicket>,
    submitting: Option<PathEditorRequestTicket>,
    reset_in_progress: bool,
    active: Option<PathEditorActiveTicket>,
    completed: Option<(PathEditorRequestTicket, T)>,
    retired_jobs: VecDeque<usize>,
}

impl<T> Default for PathEditorLifecycle<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            owner_dialog: 0,
            pending: None,
            submitting: None,
            reset_in_progress: false,
            active: None,
            completed: None,
            retired_jobs: VecDeque::with_capacity(RETIRED_JOB_CAPACITY),
        }
    }
}

impl<T> PathEditorLifecycle<T> {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn owner_dialog(&self) -> usize {
        self.owner_dialog
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn has_submit_lease(&self) -> bool {
        self.submitting.is_some()
    }

    pub fn reset_in_progress(&self) -> bool {
        self.reset_in_progress
    }

    pub fn lease_invariants_hold(&self) -> bool {
        !(self.submitting.is_some() && self.reset_in_progress)
    }

    pub fn has_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_job(&self) -> Option<usize> {
        self.active.map(|active| active.job)
    }

    pub fn active_provenance(&self) -> Option<PathEditorActiveProvenance> {
        self.active.map(|active| PathEditorActiveProvenance {
            generation: active.request.generation,
            job: active.job,
        })
    }

    pub fn recognizes_job(&self, job: usize) -> bool {
        self.active_job() == Some(job) || self.retired_jobs.contains(&job)
    }

    pub fn has_completed(&self) -> bool {
        self.completed.is_some()
    }

    fn advance_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
    }

    fn retire_job(&mut self, job: usize) {
        if job == 0 || self.retired_jobs.contains(&job) {
            return;
        }
        if self.retired_jobs.len() == RETIRED_JOB_CAPACITY {
            self.retired_jobs.pop_front();
        }
        self.retired_jobs.push_back(job);
    }

    fn invalidate_work(&mut self) -> bool {
        let had_work = self.pending.is_some() || self.active.is_some() || self.completed.is_some();
        if let Some(active) = self.active.take() {
            self.retire_job(active.job);
        }
        self.pending = None;
        self.completed = None;
        had_work
    }

    pub fn begin_reset(&mut self) -> PathEditorResetLease {
        if self.submitting.is_some() {
            return PathEditorResetLease::DeferredForSubmit;
        }
        if self.reset_in_progress {
            return PathEditorResetLease::DeferredForReset;
        }
        let invalidated = self.invalidate_work();
        self.owner_dialog = 0;
        self.advance_generation();
        self.reset_in_progress = true;
        PathEditorResetLease::Acquired { invalidated }
    }

    pub fn finish_reset(&mut self) -> bool {
        std::mem::replace(&mut self.reset_in_progress, false)
    }

    pub fn reconcile_identity(&mut self, identity: PathEditorPickerIdentity) -> bool {
        if self.submitting.is_some()
            || self.reset_in_progress
            || self.owner_dialog == 0
            || identity.owns(self.owner_dialog)
        {
            return false;
        }
        let invalidated = self.invalidate_work();
        self.owner_dialog = 0;
        self.advance_generation();
        invalidated
    }

    pub fn request(
        &mut self,
        identity: PathEditorPickerIdentity,
        dialog: usize,
    ) -> Result<PathEditorRequestTicket, PathEditorLifecycleRejection> {
        if dialog == 0 {
            return Err(PathEditorLifecycleRejection::InvalidDialog);
        }
        if self.submitting.is_some() || self.reset_in_progress {
            return Err(PathEditorLifecycleRejection::Busy);
        }
        if !identity.owns(dialog) {
            return Err(PathEditorLifecycleRejection::IdentityMismatch);
        }
        if self.owner_dialog != dialog {
            self.invalidate_work();
            self.advance_generation();
            self.owner_dialog = dialog;
        }
        if self.pending.is_some() || self.active.is_some() || self.completed.is_some() {
            return Err(PathEditorLifecycleRejection::Busy);
        }
        let ticket = PathEditorRequestTicket {
            dialog,
            generation: self.generation,
        };
        self.pending = Some(ticket);
        Ok(ticket)
    }

    pub fn begin_submit(
        &mut self,
        identity: PathEditorPickerIdentity,
    ) -> Result<Option<PathEditorRequestTicket>, PathEditorLifecycleRejection> {
        if self.reset_in_progress || self.submitting.is_some() {
            return Err(PathEditorLifecycleRejection::Busy);
        }
        let Some(ticket) = self.pending else {
            return Ok(None);
        };
        if !identity.owns(ticket.dialog)
            || ticket.dialog != self.owner_dialog
            || ticket.generation != self.generation
        {
            self.invalidate_work();
            self.owner_dialog = 0;
            self.advance_generation();
            return Err(PathEditorLifecycleRejection::IdentityMismatch);
        }
        self.submitting = Some(ticket);
        Ok(Some(ticket))
    }

    pub fn finish_submit(
        &mut self,
        ticket: PathEditorRequestTicket,
    ) -> Result<(), PathEditorLifecycleRejection> {
        if self.submitting != Some(ticket) {
            return Err(PathEditorLifecycleRejection::RequestMismatch);
        }
        self.submitting = None;
        Ok(())
    }

    pub fn reject_pending_submit(
        &mut self,
        ticket: PathEditorRequestTicket,
    ) -> Result<(), PathEditorLifecycleRejection> {
        if self.pending != Some(ticket) || self.submitting != Some(ticket) {
            return Err(PathEditorLifecycleRejection::RequestMismatch);
        }
        self.pending = None;
        Ok(())
    }

    pub fn activate(
        &mut self,
        ticket: PathEditorRequestTicket,
        job: usize,
    ) -> Result<(), PathEditorLifecycleRejection> {
        if job == 0 {
            return Err(PathEditorLifecycleRejection::InvalidJob);
        }
        if self.pending != Some(ticket)
            || self.submitting != Some(ticket)
            || self.reset_in_progress
            || ticket.dialog != self.owner_dialog
            || ticket.generation != self.generation
            || self.active.is_some()
        {
            return Err(PathEditorLifecycleRejection::RequestMismatch);
        }
        // Menu-heap allocations may reuse an address after the prior native job is destroyed.
        // A valid new submit ticket owns that fresh allocation, so reclaim its address from the
        // bounded stale-result tombstones instead of rejecting every retry forever.
        if let Some(index) = self.retired_jobs.iter().position(|retired| *retired == job) {
            self.retired_jobs.remove(index);
        }
        self.pending = None;
        self.active = Some(PathEditorActiveTicket {
            request: ticket,
            job,
        });
        Ok(())
    }

    pub fn abort_active_submit(&mut self, ticket: PathEditorRequestTicket, job: usize) -> bool {
        if self.active
            != Some(PathEditorActiveTicket {
                request: ticket,
                job,
            })
        {
            return false;
        }
        self.active = None;
        self.retire_job(job);
        true
    }

    pub fn record_result(&mut self, job: usize, outcome: T) -> PathEditorResultOwnership {
        let Some(active) = self.active else {
            return if self.retired_jobs.contains(&job) {
                PathEditorResultOwnership::StaleOwned
            } else {
                PathEditorResultOwnership::Foreign
            };
        };
        if active.job != job {
            return if self.retired_jobs.contains(&job) {
                PathEditorResultOwnership::StaleOwned
            } else {
                PathEditorResultOwnership::Foreign
            };
        }
        self.active = None;
        self.retire_job(job);
        if active.request.generation != self.generation
            || active.request.dialog != self.owner_dialog
            || self.completed.is_some()
        {
            return PathEditorResultOwnership::StaleOwned;
        }
        self.completed = Some((active.request, outcome));
        PathEditorResultOwnership::Current
    }

    fn take_completed_with_ticket_allow_owner_zero(
        &mut self,
        identity: PathEditorPickerIdentity,
        allow_owner_zero: bool,
    ) -> Result<Option<(PathEditorRequestTicket, T)>, PathEditorLifecycleRejection> {
        let Some((ticket, _)) = self.completed.as_ref() else {
            return Ok(None);
        };
        let identity_owned = identity.owns(ticket.dialog)
            || (allow_owner_zero && identity.picker_mode_active && identity.current_dialog == 0);
        if !identity_owned
            || ticket.dialog != self.owner_dialog
            || ticket.generation != self.generation
        {
            self.completed = None;
            return Err(PathEditorLifecycleRejection::IdentityMismatch);
        }
        Ok(self.completed.take())
    }

    pub fn take_completed_with_ticket(
        &mut self,
        identity: PathEditorPickerIdentity,
    ) -> Result<Option<(PathEditorRequestTicket, T)>, PathEditorLifecycleRejection> {
        self.take_completed_with_ticket_allow_owner_zero(identity, false)
    }

    pub fn take_completed_for_owner_transition(
        &mut self,
        identity: PathEditorPickerIdentity,
    ) -> Result<Option<(PathEditorRequestTicket, T)>, PathEditorLifecycleRejection> {
        self.take_completed_with_ticket_allow_owner_zero(identity, true)
    }

    pub fn take_completed(
        &mut self,
        identity: PathEditorPickerIdentity,
    ) -> Result<Option<T>, PathEditorLifecycleRejection> {
        self.take_completed_with_ticket(identity)
            .map(|completed| completed.map(|(_, outcome)| outcome))
    }
}

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEditorLifecycleStatus {
    Idle = 0,
    Pending = 1,
    Submitted = 2,
    NativeAccept = 3,
    NativeCancel = 4,
    StaleResult = 5,
    IdentityRejected = 6,
    SubmitFailed = 7,
    ValidationRejected = 8,
    AppliedDirectory = 9,
    RebuildScheduled = 10,
    Reset = 11,
    ResetDeferred = 12,
    DeferredCloseDrained = 13,
    DeferredCloseCancelled = 14,
    Submitting = 15,
    RecipeUnavailable = 16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathEditorDeferredCloseTicket {
    pub dialog: usize,
    pub generation: u64,
    pub owner: PathEditorPickerIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathEditorLifecycleSnapshot {
    pub generation: u64,
    pub owner_dialog: usize,
    pub pending: bool,
    pub submit_lease_active: bool,
    pub reset_lease_active: bool,
    pub deferred_close: Option<PathEditorDeferredCloseTicket>,
    pub status: PathEditorLifecycleStatus,
}

pub trait PathEditorLifecyclePublisher {
    fn publish(&self, snapshot: PathEditorLifecycleSnapshot);

    fn invariant_violation(&self) {}
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopPathEditorLifecyclePublisher;

impl PathEditorLifecyclePublisher for NoopPathEditorLifecyclePublisher {
    fn publish(&self, _snapshot: PathEditorLifecycleSnapshot) {}
}

#[derive(Debug)]
struct PathEditorCoordinatedState<T> {
    lifecycle: PathEditorLifecycle<T>,
    deferred_close: Option<PathEditorDeferredCloseTicket>,
    status: PathEditorLifecycleStatus,
}

impl<T> Default for PathEditorCoordinatedState<T> {
    fn default() -> Self {
        Self {
            lifecycle: PathEditorLifecycle::default(),
            deferred_close: None,
            status: PathEditorLifecycleStatus::Idle,
        }
    }
}

/// Production synchronization primitive for CurrentPath editor lifecycle, telemetry publication,
/// and deferred native close ownership. Every state mutation publishes its snapshot before the
/// lifecycle mutex is released. Native sinks run outside the mutex while an RAII submit/reset lease
/// remains active, so synchronous callbacks can reenter without deadlocking.
pub struct PathEditorCoordinator<T, P: PathEditorLifecyclePublisher> {
    state: Mutex<PathEditorCoordinatedState<T>>,
    publisher: P,
}

impl<T, P: PathEditorLifecyclePublisher> PathEditorCoordinator<T, P> {
    pub fn new(publisher: P) -> Self {
        Self {
            state: Mutex::new(PathEditorCoordinatedState::default()),
            publisher,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PathEditorCoordinatedState<T>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot_locked(state: &PathEditorCoordinatedState<T>) -> PathEditorLifecycleSnapshot {
        PathEditorLifecycleSnapshot {
            generation: state.lifecycle.generation(),
            owner_dialog: state.lifecycle.owner_dialog(),
            pending: state.lifecycle.has_pending(),
            submit_lease_active: state.lifecycle.has_submit_lease(),
            reset_lease_active: state.lifecycle.reset_in_progress(),
            deferred_close: state.deferred_close,
            status: state.status,
        }
    }

    fn publish_locked(&self, state: &PathEditorCoordinatedState<T>) {
        self.publisher.publish(Self::snapshot_locked(state));
    }

    fn close_ticket_is_current(
        state: &PathEditorCoordinatedState<T>,
        identity: PathEditorPickerIdentity,
        ticket: PathEditorDeferredCloseTicket,
    ) -> bool {
        identity == ticket.owner
            && identity.owns(ticket.dialog)
            && state.lifecycle.owner_dialog() == ticket.dialog
            && state.lifecycle.generation() == ticket.generation
    }

    pub fn snapshot(&self) -> PathEditorLifecycleSnapshot {
        let state = self.lock();
        Self::snapshot_locked(&state)
    }

    pub fn set_status(&self, status: PathEditorLifecycleStatus) {
        let mut state = self.lock();
        state.status = status;
        self.publish_locked(&state);
    }

    pub fn request(
        &self,
        identity: PathEditorPickerIdentity,
        dialog: usize,
    ) -> Result<PathEditorRequestTicket, PathEditorLifecycleRejection> {
        let mut state = self.lock();
        if state.deferred_close.is_some() {
            state.status = PathEditorLifecycleStatus::ResetDeferred;
            self.publish_locked(&state);
            return Err(PathEditorLifecycleRejection::Busy);
        }
        let result = state.lifecycle.request(identity, dialog);
        state.status = if result.is_ok() {
            PathEditorLifecycleStatus::Pending
        } else {
            PathEditorLifecycleStatus::IdentityRejected
        };
        self.publish_locked(&state);
        result
    }

    pub fn recognizes_job(&self, job: usize) -> bool {
        self.lock().lifecycle.recognizes_job(job)
    }

    pub fn active_job(&self) -> Option<usize> {
        self.lock().lifecycle.active_job()
    }

    /// Read the exact active generation/job pair under one lifecycle mutex acquisition.
    pub fn active_provenance(&self) -> Option<PathEditorActiveProvenance> {
        self.lock().lifecycle.active_provenance()
    }

    /// Publish a terminal result only when `expected` is still the exact active generation/job pair,
    /// while holding the lifecycle mutex. Window-lifetime observers use this instead of comparing a
    /// separately-read address, which could retire a newer generation after heap-address reuse.
    pub fn record_active_result(
        &self,
        expected: PathEditorActiveProvenance,
        outcome: T,
        current_status: PathEditorLifecycleStatus,
    ) -> Option<PathEditorResultOwnership> {
        let mut state = self.lock();
        if state.lifecycle.active_provenance() != Some(expected) {
            return None;
        }
        let ownership = state.lifecycle.record_result(expected.job, outcome);
        state.status = match ownership {
            PathEditorResultOwnership::Current => current_status,
            PathEditorResultOwnership::StaleOwned => PathEditorLifecycleStatus::StaleResult,
            PathEditorResultOwnership::Foreign => state.status,
        };
        self.publish_locked(&state);
        Some(ownership)
    }

    pub fn abort_active_submit(&self, ticket: PathEditorRequestTicket, job: usize) -> bool {
        let mut state = self.lock();
        let aborted = state.lifecycle.abort_active_submit(ticket, job);
        if aborted {
            state.status = PathEditorLifecycleStatus::StaleResult;
        }
        self.publish_locked(&state);
        aborted
    }

    pub fn record_result(
        &self,
        job: usize,
        outcome: T,
        current_status: PathEditorLifecycleStatus,
    ) -> PathEditorResultOwnership {
        let mut state = self.lock();
        let ownership = state.lifecycle.record_result(job, outcome);
        state.status = match ownership {
            PathEditorResultOwnership::Current => current_status,
            PathEditorResultOwnership::StaleOwned => PathEditorLifecycleStatus::StaleResult,
            PathEditorResultOwnership::Foreign => state.status,
        };
        self.publish_locked(&state);
        ownership
    }

    pub fn take_completed_with_ticket(
        &self,
        identity: PathEditorPickerIdentity,
    ) -> Result<Option<(PathEditorRequestTicket, T)>, PathEditorLifecycleRejection> {
        let mut state = self.lock();
        let result = state.lifecycle.take_completed_with_ticket(identity);
        if result.is_err() {
            state.status = PathEditorLifecycleStatus::StaleResult;
        }
        self.publish_locked(&state);
        result
    }

    pub fn take_completed_for_owner_transition(
        &self,
        identity: PathEditorPickerIdentity,
    ) -> Result<Option<(PathEditorRequestTicket, T)>, PathEditorLifecycleRejection> {
        let mut state = self.lock();
        let result = state
            .lifecycle
            .take_completed_for_owner_transition(identity);
        if result.is_err() {
            state.status = PathEditorLifecycleStatus::StaleResult;
        }
        self.publish_locked(&state);
        result
    }

    pub fn take_completed(
        &self,
        identity: PathEditorPickerIdentity,
    ) -> Result<Option<T>, PathEditorLifecycleRejection> {
        self.take_completed_with_ticket(identity)
            .map(|completed| completed.map(|(_, outcome)| outcome))
    }

    /// Validate and apply an already-acquired terminal result under the same lifecycle mutex used
    /// by owner publication. Lock order for callers is lifecycle -> retained model -> path-return
    /// state -> refresh state. The transaction must not invoke native code or call back into this
    /// coordinator. A newer/different/same-address-new-generation publication therefore either wins
    /// before this lock and rejects the ticket, or waits until mutation+return-arm is indivisible.
    pub fn with_terminal_result_transaction<R>(
        &self,
        identity: PathEditorPickerIdentity,
        ticket: PathEditorRequestTicket,
        transaction: impl FnOnce() -> R,
    ) -> Result<PathEditorTerminalTransaction<R>, PathEditorLifecycleRejection> {
        let mut state = self.lock();
        let exact_generation = state.lifecycle.generation() == ticket.generation
            && state.lifecycle.owner_dialog() == ticket.dialog;
        let exact_identity = identity.picker_mode_active
            && (identity.current_dialog == ticket.dialog || identity.current_dialog == 0);
        if !exact_generation || !exact_identity {
            state.status = PathEditorLifecycleStatus::StaleResult;
            self.publish_locked(&state);
            return Err(PathEditorLifecycleRejection::RequestMismatch);
        }
        let result = transaction();
        let cancelled_close = state
            .deferred_close
            .filter(|close| !Self::close_ticket_is_current(&state, identity, *close));
        if cancelled_close.is_some() {
            state.deferred_close = None;
            state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
        }
        let invalidated = state.lifecycle.reconcile_identity(identity);
        if invalidated && cancelled_close.is_none() {
            state.status = PathEditorLifecycleStatus::IdentityRejected;
        }
        self.publish_locked(&state);
        Ok(PathEditorTerminalTransaction {
            result,
            reconcile: PathEditorReconcile {
                invalidated,
                cancelled_close,
            },
        })
    }

    /// Reconcile the lifecycle to the exact currently-published picker identity. If native state
    /// already stopped matching a deferred close ticket, cancel the ticket without invoking any
    /// native sink; it is no longer safe to dereference.
    pub fn reconcile_identity(&self, identity: PathEditorPickerIdentity) -> PathEditorReconcile {
        let mut state = self.lock();
        let cancelled_close = state
            .deferred_close
            .filter(|ticket| !Self::close_ticket_is_current(&state, identity, *ticket));
        if cancelled_close.is_some() {
            state.deferred_close = None;
            state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
        }
        let invalidated = state.lifecycle.reconcile_identity(identity);
        if invalidated && cancelled_close.is_none() {
            state.status = PathEditorLifecycleStatus::IdentityRejected;
        }
        self.publish_locked(&state);
        PathEditorReconcile {
            invalidated,
            cancelled_close,
        }
    }

    /// Publish the native ProfileSelect owner under the same mutex/order as lifecycle identity.
    /// A different owner cannot replace the exact owner of an undrained close ticket.
    pub fn publish_owner<R>(
        &self,
        picker_mode_active: bool,
        current_dialog: usize,
        new_dialog: usize,
        publish: impl FnOnce(usize) -> R,
    ) -> PathEditorOwnerPublication<R> {
        let mut state = self.lock();
        let current_identity = PathEditorPickerIdentity {
            picker_mode_active,
            current_dialog,
        };
        let cancelled_close = state.deferred_close.filter(|ticket| {
            new_dialog != ticket.dialog
                || !Self::close_ticket_is_current(&state, current_identity, *ticket)
        });
        if cancelled_close.is_some() {
            state.deferred_close = None;
            state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
        }
        let result = publish(new_dialog);
        let identity = PathEditorPickerIdentity {
            picker_mode_active,
            current_dialog: new_dialog,
        };
        if state.lifecycle.reconcile_identity(identity) {
            state.status = PathEditorLifecycleStatus::IdentityRejected;
        }
        self.publish_locked(&state);
        PathEditorOwnerPublication::Published {
            result,
            cancelled_close,
            generation: state.lifecycle.generation(),
        }
    }

    /// Compare-and-publish native owner identity under the same lifecycle mutex used by every
    /// normal owner publication. A stale game-task absence observation therefore cannot clear a
    /// newer MenuWindow-post owner between its read and atomic publication.
    pub fn publish_owner_if_current<R>(
        &self,
        picker_mode_active: bool,
        expected_dialog: usize,
        new_dialog: usize,
        publish: impl FnOnce(usize, usize) -> Result<R, R>,
    ) -> PathEditorOwnerComparePublication<R> {
        let mut state = self.lock();
        let result = match publish(expected_dialog, new_dialog) {
            Ok(result) => result,
            Err(actual) => return PathEditorOwnerComparePublication::Stale { actual },
        };
        let current_identity = PathEditorPickerIdentity {
            picker_mode_active,
            current_dialog: expected_dialog,
        };
        let cancelled_close = state.deferred_close.filter(|ticket| {
            new_dialog != ticket.dialog
                || !Self::close_ticket_is_current(&state, current_identity, *ticket)
        });
        if cancelled_close.is_some() {
            state.deferred_close = None;
            state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
        }
        let identity = PathEditorPickerIdentity {
            picker_mode_active,
            current_dialog: new_dialog,
        };
        if state.lifecycle.reconcile_identity(identity) {
            state.status = PathEditorLifecycleStatus::IdentityRejected;
        }
        self.publish_locked(&state);
        PathEditorOwnerComparePublication::Published {
            result,
            cancelled_close,
        }
    }

    pub fn with_submit<R>(
        &self,
        identity: PathEditorPickerIdentity,
        submit: impl FnOnce(PathEditorRequestTicket, &Self) -> R,
    ) -> Result<Option<R>, PathEditorLifecycleRejection> {
        let ticket = {
            let mut state = self.lock();
            if state.deferred_close.is_some() {
                state.status = PathEditorLifecycleStatus::ResetDeferred;
                self.publish_locked(&state);
                return Err(PathEditorLifecycleRejection::Busy);
            }
            let result = state.lifecycle.begin_submit(identity);
            if result.is_ok_and(|ticket| ticket.is_some()) {
                state.status = PathEditorLifecycleStatus::Submitting;
            } else if result.is_err() {
                state.status = PathEditorLifecycleStatus::IdentityRejected;
            }
            self.publish_locked(&state);
            result?
        };
        let Some(ticket) = ticket else {
            return Ok(None);
        };
        let guard = PathEditorSubmitGuard {
            coordinator: self,
            ticket,
        };
        let result = submit(ticket, self);
        drop(guard);
        Ok(Some(result))
    }

    /// Retire the exact pending request while its submit lease is held. This is for terminal
    /// pre-submit failures only; retryable native readiness failures leave `pending` intact.
    pub fn reject_pending_submit(
        &self,
        ticket: PathEditorRequestTicket,
        status: PathEditorLifecycleStatus,
    ) -> Result<(), PathEditorLifecycleRejection> {
        let mut state = self.lock();
        let result = state.lifecycle.reject_pending_submit(ticket);
        state.status = if result.is_ok() {
            status
        } else {
            PathEditorLifecycleStatus::IdentityRejected
        };
        self.publish_locked(&state);
        result
    }

    pub fn activate(
        &self,
        ticket: PathEditorRequestTicket,
        job: usize,
    ) -> Result<(), PathEditorLifecycleRejection> {
        let mut state = self.lock();
        let result = state.lifecycle.activate(ticket, job);
        state.status = if result.is_ok() {
            PathEditorLifecycleStatus::Submitted
        } else {
            PathEditorLifecycleStatus::IdentityRejected
        };
        self.publish_locked(&state);
        result
    }

    fn finish_submit(&self, ticket: PathEditorRequestTicket) -> bool {
        let mut state = self.lock();
        let ok = state.lifecycle.finish_submit(ticket).is_ok()
            && state.lifecycle.lease_invariants_hold();
        if !ok {
            self.publisher.invariant_violation();
        }
        self.publish_locked(&state);
        ok
    }

    pub fn begin_reset(
        &self,
        identity: PathEditorPickerIdentity,
    ) -> PathEditorResetStart<'_, T, P> {
        let mut state = self.lock();
        let mut cancelled_close = None;
        if let Some(ticket) = state.deferred_close {
            if Self::close_ticket_is_current(&state, identity, ticket) {
                state.status = PathEditorLifecycleStatus::ResetDeferred;
                self.publish_locked(&state);
                return PathEditorResetStart::OwnedCloseMustDrain(ticket);
            }
            state.deferred_close = None;
            cancelled_close = Some(ticket);
        }
        match state.lifecycle.begin_reset() {
            PathEditorResetLease::Acquired { invalidated } => {
                state.status = PathEditorLifecycleStatus::Reset;
                self.publish_locked(&state);
                drop(state);
                PathEditorResetStart::Acquired {
                    guard: PathEditorResetGuard { coordinator: self },
                    invalidated,
                    cancelled_close,
                }
            }
            PathEditorResetLease::DeferredForSubmit => {
                state.status = PathEditorLifecycleStatus::ResetDeferred;
                self.publish_locked(&state);
                PathEditorResetStart::DeferredForSubmit
            }
            PathEditorResetLease::DeferredForReset => {
                state.status = PathEditorLifecycleStatus::ResetDeferred;
                self.publish_locked(&state);
                PathEditorResetStart::DeferredForReset
            }
        }
    }

    pub fn close_with(
        &self,
        identity: PathEditorPickerIdentity,
        dialog: usize,
        close: impl FnOnce(usize) -> bool,
    ) -> PathEditorCloseDisposition {
        let mut state = self.lock();
        if !identity.owns(dialog) {
            state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
            self.publish_locked(&state);
            return PathEditorCloseDisposition::Cancelled(None);
        }
        if state.lifecycle.has_submit_lease() {
            let ticket = PathEditorDeferredCloseTicket {
                dialog,
                generation: state.lifecycle.generation(),
                owner: identity,
            };
            if state.lifecycle.owner_dialog() != dialog {
                state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
                self.publish_locked(&state);
                return PathEditorCloseDisposition::Cancelled(Some(ticket));
            }
            state.deferred_close = Some(ticket);
            state.status = PathEditorLifecycleStatus::ResetDeferred;
            self.publish_locked(&state);
            return PathEditorCloseDisposition::Deferred(ticket);
        }
        if state.lifecycle.reset_in_progress() {
            state.status = PathEditorLifecycleStatus::ResetDeferred;
            self.publish_locked(&state);
            return PathEditorCloseDisposition::ResetInProgress;
        }
        if let Some(ticket) = state.deferred_close.take() {
            if !Self::close_ticket_is_current(&state, identity, ticket) || ticket.dialog != dialog {
                state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
                self.publish_locked(&state);
                return PathEditorCloseDisposition::Cancelled(Some(ticket));
            }
        }
        let invalidated = match state.lifecycle.begin_reset() {
            PathEditorResetLease::Acquired { invalidated } => invalidated,
            PathEditorResetLease::DeferredForSubmit => unreachable!("submit lease checked above"),
            PathEditorResetLease::DeferredForReset => unreachable!("reset lease checked above"),
        };
        state.status = PathEditorLifecycleStatus::Reset;
        self.publish_locked(&state);
        drop(state);
        let guard = PathEditorResetGuard { coordinator: self };
        let closed = close(dialog);
        drop(guard);
        PathEditorCloseDisposition::Closed {
            closed,
            invalidated,
        }
    }

    pub fn retry_deferred_close(
        &self,
        identity: PathEditorPickerIdentity,
        close: impl FnOnce(usize) -> bool,
    ) -> PathEditorDeferredCloseDisposition {
        let mut state = self.lock();
        let Some(ticket) = state.deferred_close else {
            return PathEditorDeferredCloseDisposition::None;
        };
        if state.lifecycle.has_submit_lease() || state.lifecycle.reset_in_progress() {
            state.status = PathEditorLifecycleStatus::ResetDeferred;
            self.publish_locked(&state);
            return PathEditorDeferredCloseDisposition::Deferred(ticket);
        }
        state.deferred_close = None;
        if !Self::close_ticket_is_current(&state, identity, ticket) {
            state.status = PathEditorLifecycleStatus::DeferredCloseCancelled;
            self.publish_locked(&state);
            return PathEditorDeferredCloseDisposition::Cancelled(ticket);
        }
        match state.lifecycle.begin_reset() {
            PathEditorResetLease::Acquired { .. } => {}
            PathEditorResetLease::DeferredForSubmit => unreachable!("submit lease checked above"),
            PathEditorResetLease::DeferredForReset => unreachable!("reset lease checked above"),
        }
        state.status = PathEditorLifecycleStatus::Reset;
        self.publish_locked(&state);
        drop(state);
        let guard = PathEditorResetGuard { coordinator: self };
        let closed = close(ticket.dialog);
        self.set_status(PathEditorLifecycleStatus::DeferredCloseDrained);
        drop(guard);
        PathEditorDeferredCloseDisposition::Drained { ticket, closed }
    }
}

pub struct PathEditorSubmitGuard<'a, T, P: PathEditorLifecyclePublisher> {
    coordinator: &'a PathEditorCoordinator<T, P>,
    ticket: PathEditorRequestTicket,
}

impl<T, P: PathEditorLifecyclePublisher> Drop for PathEditorSubmitGuard<'_, T, P> {
    fn drop(&mut self) {
        let _ = self.coordinator.finish_submit(self.ticket);
    }
}

pub struct PathEditorResetGuard<'a, T, P: PathEditorLifecyclePublisher> {
    coordinator: &'a PathEditorCoordinator<T, P>,
}

impl<T, P: PathEditorLifecyclePublisher> Drop for PathEditorResetGuard<'_, T, P> {
    fn drop(&mut self) {
        let mut state = self.coordinator.lock();
        let ok = state.lifecycle.finish_reset() && state.lifecycle.lease_invariants_hold();
        if !ok {
            self.coordinator.publisher.invariant_violation();
        }
        self.coordinator.publish_locked(&state);
    }
}

pub struct PathEditorTerminalTransaction<R> {
    pub result: R,
    pub reconcile: PathEditorReconcile,
}

pub struct PathEditorReconcile {
    pub invalidated: bool,
    pub cancelled_close: Option<PathEditorDeferredCloseTicket>,
}

pub enum PathEditorOwnerPublication<R> {
    Published {
        result: R,
        cancelled_close: Option<PathEditorDeferredCloseTicket>,
        generation: u64,
    },
}

pub enum PathEditorOwnerComparePublication<R> {
    Published {
        result: R,
        cancelled_close: Option<PathEditorDeferredCloseTicket>,
    },
    Stale {
        actual: R,
    },
}

pub enum PathEditorResetStart<'a, T, P: PathEditorLifecyclePublisher> {
    Acquired {
        guard: PathEditorResetGuard<'a, T, P>,
        invalidated: bool,
        cancelled_close: Option<PathEditorDeferredCloseTicket>,
    },
    DeferredForSubmit,
    DeferredForReset,
    OwnedCloseMustDrain(PathEditorDeferredCloseTicket),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEditorCloseDisposition {
    Closed { closed: bool, invalidated: bool },
    Deferred(PathEditorDeferredCloseTicket),
    ResetInProgress,
    Cancelled(Option<PathEditorDeferredCloseTicket>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEditorDeferredCloseDisposition {
    None,
    Deferred(PathEditorDeferredCloseTicket),
    Drained {
        ticket: PathEditorDeferredCloseTicket,
        closed: bool,
    },
    Cancelled(PathEditorDeferredCloseTicket),
}

/// Production menu-tick gate. `Drained` is distinct from `NoTicket` so the tick that invoked a
/// native close also stops: reopening waits for a later owner observation/tick rather than racing
/// the close callback. `Deferred` stops the tick until submit/reset ownership releases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEditorCloseRetryGate {
    NoTicket,
    Drained,
    Deferred,
}

/// Run the picker-owned portion of one menu tick only when no deferred close/reset transaction is
/// outstanding. Production and deterministic tests share this exact ordering seam.
pub fn run_picker_tick_after_close_gate(
    retry: impl FnOnce() -> PathEditorCloseRetryGate,
    open_destination: impl FnOnce(),
    pump_path_editor: impl FnOnce(),
    rebuild: impl FnOnce(),
    resubmit: impl FnOnce(),
) -> PathEditorCloseRetryGate {
    let gate = retry();
    if gate != PathEditorCloseRetryGate::NoTicket {
        return gate;
    }
    open_destination();
    pump_path_editor();
    rebuild();
    resubmit();
    PathEditorCloseRetryGate::NoTicket
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickedFileCloseRoute {
    ClosedAndCleared,
    Deferred,
}

/// Arm the eventual slot-view transition, request/perform the owned close, and only then clear
/// picker mode/model. A deferred close leaves mode intact; its drain path performs the clear later.
pub fn run_picked_file_close_route(
    arm_open_slots: impl FnOnce(),
    close: impl FnOnce() -> bool,
    clear_picker_mode: impl FnOnce(),
) -> PickedFileCloseRoute {
    arm_open_slots();
    if !close() {
        return PickedFileCloseRoute::Deferred;
    }
    clear_picker_mode();
    PickedFileCloseRoute::ClosedAndCleared
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIALOG_A: usize = 0x5000;
    const DIALOG_B: usize = 0x6000;
    const JOB_A: usize = 0x7000;
    const JOB_B: usize = 0x8000;

    fn identity(dialog: usize) -> PathEditorPickerIdentity {
        PathEditorPickerIdentity {
            picker_mode_active: true,
            current_dialog: dialog,
        }
    }

    #[test]
    fn close_before_submit_clears_pending_request_and_advances_generation() {
        let mut state = PathEditorLifecycle::<&'static str>::default();
        let request = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert!(state.has_pending());
        assert_eq!(
            state.begin_reset(),
            PathEditorResetLease::Acquired { invalidated: true }
        );
        assert!(!state.has_pending());
        assert!(state.generation() > request.generation);
        assert!(state.finish_reset());
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(None));
    }

    #[test]
    fn reset_invalidates_active_job_and_late_result_fails_closed() {
        let mut state = PathEditorLifecycle::default();
        let request = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(request)));
        state.activate(request, JOB_A).unwrap();
        state.finish_submit(request).unwrap();
        assert_eq!(
            state.begin_reset(),
            PathEditorResetLease::Acquired { invalidated: true }
        );
        assert!(state.finish_reset());
        assert_eq!(
            state.record_result(JOB_A, "late"),
            PathEditorResultOwnership::StaleOwned
        );
        assert!(!state.has_completed());
        assert_eq!(state.take_completed(identity(DIALOG_A)), Ok(None));
    }

    #[test]
    fn dialog_identity_loss_clears_pending_and_rejects_old_ticket() {
        let mut state = PathEditorLifecycle::<&'static str>::default();
        let request = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert!(state.reconcile_identity(PathEditorPickerIdentity {
            picker_mode_active: false,
            current_dialog: 0,
        }));
        assert_eq!(
            state.activate(request, JOB_A),
            Err(PathEditorLifecycleRejection::RequestMismatch)
        );
        assert!(!state.has_pending());
        assert!(!state.has_active());
    }

    #[test]
    fn reopen_uses_new_generation_and_cannot_apply_old_result() {
        let mut state = PathEditorLifecycle::default();
        let first = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(first)));
        state.activate(first, JOB_A).unwrap();
        state.finish_submit(first).unwrap();
        assert_eq!(
            state.begin_reset(),
            PathEditorResetLease::Acquired { invalidated: true }
        );
        assert!(state.finish_reset());
        let second = state.request(identity(DIALOG_B), DIALOG_B).unwrap();
        assert!(second.generation > first.generation);
        assert_eq!(state.begin_submit(identity(DIALOG_B)), Ok(Some(second)));
        state.activate(second, JOB_B).unwrap();
        state.finish_submit(second).unwrap();
        assert_eq!(
            state.record_result(JOB_A, "old"),
            PathEditorResultOwnership::StaleOwned
        );
        assert_eq!(
            state.record_result(JOB_B, "new"),
            PathEditorResultOwnership::Current
        );
        assert_eq!(state.take_completed(identity(DIALOG_B)), Ok(Some("new")));
    }

    #[test]
    fn reopened_submit_reclaims_a_retired_job_address() {
        let mut state = PathEditorLifecycle::default();
        let first = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(first)));
        state.activate(first, JOB_A).unwrap();
        state.finish_submit(first).unwrap();
        assert_eq!(
            state.record_result(JOB_A, "first"),
            PathEditorResultOwnership::Current
        );
        assert_eq!(state.take_completed(identity(DIALOG_A)), Ok(Some("first")));
        assert!(state.recognizes_job(JOB_A));

        let second = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(second)));
        state.activate(second, JOB_A).unwrap();
        state.finish_submit(second).unwrap();
        assert_eq!(state.active_job(), Some(JOB_A));
        assert_eq!(
            state.record_result(JOB_A, "second"),
            PathEditorResultOwnership::Current
        );
        assert_eq!(state.take_completed(identity(DIALOG_A)), Ok(Some("second")));
    }

    #[test]
    fn duplicate_result_is_stale_and_does_not_replace_first_outcome() {
        let mut state = PathEditorLifecycle::default();
        let request = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(request)));
        state.activate(request, JOB_A).unwrap();
        state.finish_submit(request).unwrap();
        assert_eq!(
            state.record_result(JOB_A, "first"),
            PathEditorResultOwnership::Current
        );
        assert_eq!(
            state.record_result(JOB_A, "duplicate"),
            PathEditorResultOwnership::StaleOwned
        );
        assert_eq!(state.take_completed(identity(DIALOG_A)), Ok(Some("first")));
    }

    #[test]
    fn active_dialog_and_job_are_cleared_after_current_result_is_consumed() {
        let mut state = PathEditorLifecycle::default();
        let request = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(request)));
        state.activate(request, JOB_A).unwrap();
        state.finish_submit(request).unwrap();
        assert_eq!(state.active_job(), Some(JOB_A));
        assert_eq!(
            state.record_result(JOB_A, "done"),
            PathEditorResultOwnership::Current
        );
        assert!(!state.has_active());
        assert_eq!(state.take_completed(identity(DIALOG_A)), Ok(Some("done")));
        assert!(!state.has_completed());
    }

    #[test]
    fn menu_submit_gate_requires_mode_exact_dialog_and_generation() {
        let mut state = PathEditorLifecycle::<&'static str>::default();
        state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(
            state.begin_submit(identity(DIALOG_B)),
            Err(PathEditorLifecycleRejection::IdentityMismatch)
        );
        assert!(!state.has_pending());
    }

    #[test]
    fn terminal_submit_rejection_retires_pending_while_retryable_failure_does_not() {
        let mut state = PathEditorLifecycle::<&'static str>::default();
        let terminal = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(terminal)));
        state.reject_pending_submit(terminal).unwrap();
        state.finish_submit(terminal).unwrap();
        assert!(!state.has_pending());
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(None));

        let retryable = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(retryable)));
        state.finish_submit(retryable).unwrap();
        assert!(state.has_pending());
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(retryable)));
        state.finish_submit(retryable).unwrap();
    }

    #[test]
    fn reset_is_deferred_between_submit_validation_and_first_dialog_use() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;

        let state = Arc::new(Mutex::new(PathEditorLifecycle::<&'static str>::default()));
        let ticket = state
            .lock()
            .unwrap()
            .request(identity(DIALOG_A), DIALOG_A)
            .unwrap();
        let validated = Arc::new(Barrier::new(2));
        let reset_attempted = Arc::new(Barrier::new(2));
        let submit_released = Arc::new(Barrier::new(2));
        let dialog_dereferences = Arc::new(AtomicUsize::new(0));

        let reset_state = Arc::clone(&state);
        let reset_validated = Arc::clone(&validated);
        let reset_attempted_thread = Arc::clone(&reset_attempted);
        let reset_submit_released = Arc::clone(&submit_released);
        let reset_thread = thread::spawn(move || {
            reset_validated.wait();
            assert_eq!(
                reset_state.lock().unwrap().begin_reset(),
                PathEditorResetLease::DeferredForSubmit
            );
            reset_attempted_thread.wait();
            reset_submit_released.wait();
            let mut state = reset_state.lock().unwrap();
            assert_eq!(
                state.begin_reset(),
                PathEditorResetLease::Acquired { invalidated: true }
            );
            assert!(state.finish_reset());
        });

        assert_eq!(
            state.lock().unwrap().begin_submit(identity(DIALOG_A)),
            Ok(Some(ticket))
        );
        validated.wait();
        reset_attempted.wait();
        assert_eq!(dialog_dereferences.load(Ordering::SeqCst), 0);
        {
            let state = state.lock().unwrap();
            assert!(state.has_submit_lease());
            assert_eq!(state.owner_dialog(), DIALOG_A);
            assert_eq!(state.generation(), ticket.generation);
            assert!(state.has_pending());
        }
        dialog_dereferences.fetch_add(1, Ordering::SeqCst);
        state.lock().unwrap().finish_submit(ticket).unwrap();
        submit_released.wait();
        reset_thread.join().unwrap();

        let state = state.lock().unwrap();
        assert_eq!(dialog_dereferences.load(Ordering::SeqCst), 1);
        assert!(!state.has_submit_lease());
        assert!(!state.reset_in_progress());
        assert!(!state.has_pending());
        assert!(state.generation() > ticket.generation);
        assert!(state.lease_invariants_hold());
    }

    #[test]
    fn reentrant_result_and_reset_defer_without_deadlock_then_clean_generation() {
        let mut state = PathEditorLifecycle::default();
        let ticket = state.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert_eq!(state.begin_submit(identity(DIALOG_A)), Ok(Some(ticket)));
        state.activate(ticket, JOB_A).unwrap();

        assert_eq!(
            state.record_result(JOB_A, "synchronous callback"),
            PathEditorResultOwnership::Current
        );
        assert_eq!(state.begin_reset(), PathEditorResetLease::DeferredForSubmit);
        assert!(state.lease_invariants_hold());

        state.finish_submit(ticket).unwrap();
        assert_eq!(
            state.begin_reset(),
            PathEditorResetLease::Acquired { invalidated: true }
        );
        assert_eq!(state.begin_reset(), PathEditorResetLease::DeferredForReset);
        assert!(state.finish_reset());
        assert!(!state.has_submit_lease());
        assert!(!state.reset_in_progress());
        assert!(!state.has_active());
        assert!(!state.has_completed());
        assert!(state.generation() > ticket.generation);
        assert!(state.lease_invariants_hold());
    }

    #[derive(Clone, Default)]
    struct TestPublisher {
        last: std::sync::Arc<Mutex<Option<PathEditorLifecycleSnapshot>>>,
    }

    impl PathEditorLifecyclePublisher for TestPublisher {
        fn publish(&self, snapshot: PathEditorLifecycleSnapshot) {
            *self.last.lock().unwrap() = Some(snapshot);
        }
    }

    fn coordinator() -> (
        std::sync::Arc<PathEditorCoordinator<&'static str, TestPublisher>>,
        TestPublisher,
    ) {
        let publisher = TestPublisher::default();
        (
            std::sync::Arc::new(PathEditorCoordinator::new(publisher.clone())),
            publisher,
        )
    }

    fn assert_clean(snapshot: PathEditorLifecycleSnapshot) {
        assert!(!snapshot.submit_lease_active);
        assert!(!snapshot.reset_lease_active);
        assert!(snapshot.deferred_close.is_none());
    }

    #[test]
    fn coordinated_active_result_requires_exact_generation_and_job() {
        let (coordinator, _) = coordinator();
        coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |ticket, coordinator| {
                coordinator.activate(ticket, JOB_A).unwrap();
            })
            .unwrap();
        assert_eq!(coordinator.active_job(), Some(JOB_A));
        let active = coordinator.active_provenance().unwrap();
        assert_eq!(
            coordinator.record_active_result(
                PathEditorActiveProvenance {
                    generation: active.generation.wrapping_add(1),
                    job: active.job,
                },
                "wrong generation",
                PathEditorLifecycleStatus::NativeCancel
            ),
            None
        );
        assert_eq!(coordinator.active_provenance(), Some(active));
        assert_eq!(
            coordinator.record_active_result(
                PathEditorActiveProvenance {
                    generation: active.generation,
                    job: JOB_B,
                },
                "wrong job",
                PathEditorLifecycleStatus::NativeCancel
            ),
            None
        );
        assert_eq!(coordinator.active_provenance(), Some(active));

        assert_eq!(
            coordinator.record_active_result(
                active,
                "cancelled",
                PathEditorLifecycleStatus::NativeCancel
            ),
            Some(PathEditorResultOwnership::Current)
        );
        assert_eq!(coordinator.active_job(), None);
        assert_eq!(
            coordinator.snapshot().status,
            PathEditorLifecycleStatus::NativeCancel
        );
        assert_eq!(
            coordinator.take_completed(identity(DIALOG_A)),
            Ok(Some("cancelled"))
        );
        assert_eq!(
            coordinator.record_active_result(
                active,
                "duplicate",
                PathEditorLifecycleStatus::NativeCancel
            ),
            None
        );
    }

    #[test]
    fn coordinated_submit_pause_defers_close_then_owned_drain_precedes_clear() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};
        use std::thread;

        let (coordinator, publisher) = coordinator();
        coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        let submit_entered = Arc::new(Barrier::new(2));
        let submit_release = Arc::new(Barrier::new(2));
        let worker_coordinator = Arc::clone(&coordinator);
        let worker_entered = Arc::clone(&submit_entered);
        let worker_release = Arc::clone(&submit_release);
        let worker = thread::spawn(move || {
            worker_coordinator
                .with_submit(identity(DIALOG_A), |_, _| {
                    worker_entered.wait();
                    worker_release.wait();
                })
                .unwrap();
        });

        submit_entered.wait();
        assert!(matches!(
            coordinator.close_with(identity(DIALOG_A), DIALOG_A, |_| panic!(
                "deferred close dereferenced dialog"
            )),
            PathEditorCloseDisposition::Deferred(PathEditorDeferredCloseTicket {
                dialog: DIALOG_A,
                ..
            })
        ));
        assert!(matches!(
            coordinator.begin_reset(identity(DIALOG_A)),
            PathEditorResetStart::OwnedCloseMustDrain(_)
        ));
        submit_release.wait();
        worker.join().unwrap();

        let close_calls = AtomicUsize::new(0);
        assert!(matches!(
            coordinator.retry_deferred_close(identity(DIALOG_A), |dialog| {
                assert_eq!(dialog, DIALOG_A);
                close_calls.fetch_add(1, Ordering::SeqCst);
                true
            }),
            PathEditorDeferredCloseDisposition::Drained { closed: true, .. }
        ));
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);
        let snapshot = publisher.last.lock().unwrap().unwrap();
        assert_clean(snapshot);
        assert!(!snapshot.pending);
    }

    #[test]
    fn deferred_d1_then_observed_d2_cancels_without_stale_d1_dereference() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (coordinator, publisher) = coordinator();
        let owner = AtomicUsize::new(DIALOG_A);
        coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |_, coordinator| {
                assert!(matches!(
                    coordinator.close_with(identity(DIALOG_A), DIALOG_A, |_| panic!(
                        "deferred close called sink"
                    )),
                    PathEditorCloseDisposition::Deferred(_)
                ));
                assert!(matches!(
                    coordinator.publish_owner(
                        true,
                        owner.load(Ordering::SeqCst),
                        DIALOG_B,
                        |new| { owner.swap(new, Ordering::SeqCst) }
                    ),
                    PathEditorOwnerPublication::Published {
                        result: DIALOG_A,
                        cancelled_close: Some(PathEditorDeferredCloseTicket {
                            dialog: DIALOG_A,
                            ..
                        }),
                        ..
                    }
                ));
                assert_eq!(owner.load(Ordering::SeqCst), DIALOG_B);
            })
            .unwrap();

        let close_calls = AtomicUsize::new(0);
        assert_eq!(
            coordinator.retry_deferred_close(identity(DIALOG_B), |_| {
                close_calls.fetch_add(1, Ordering::SeqCst);
                true
            }),
            PathEditorDeferredCloseDisposition::None
        );
        assert_eq!(close_calls.load(Ordering::SeqCst), 0);
        coordinator.reconcile_identity(identity(DIALOG_B));
        assert_eq!(owner.load(Ordering::SeqCst), DIALOG_B);
        let snapshot = publisher.last.lock().unwrap().unwrap();
        assert_eq!(snapshot.status, PathEditorLifecycleStatus::IdentityRejected);
        assert_clean(snapshot);
    }

    #[test]
    fn deferred_close_ownership_mismatch_cancels_without_native_sink() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (coordinator, publisher) = coordinator();
        coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |_, coordinator| {
                assert!(matches!(
                    coordinator.close_with(identity(DIALOG_A), DIALOG_A, |_| panic!(
                        "deferred close called sink"
                    )),
                    PathEditorCloseDisposition::Deferred(_)
                ));
            })
            .unwrap();
        let reconciled = coordinator.reconcile_identity(identity(DIALOG_B));
        assert!(reconciled.cancelled_close.is_some());
        let close_calls = AtomicUsize::new(0);
        assert_eq!(
            coordinator.retry_deferred_close(identity(DIALOG_B), |_| {
                close_calls.fetch_add(1, Ordering::SeqCst);
                true
            }),
            PathEditorDeferredCloseDisposition::None
        );
        assert_eq!(close_calls.load(Ordering::SeqCst), 0);
        let snapshot = publisher.last.lock().unwrap().unwrap();
        assert_eq!(
            snapshot.status,
            PathEditorLifecycleStatus::DeferredCloseCancelled
        );
        assert_clean(snapshot);
    }

    #[test]
    fn synchronous_result_and_reset_reentry_do_not_deadlock_raii_wrappers() {
        let (coordinator, publisher) = coordinator();
        let ticket = coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |submit_ticket, coordinator| {
                assert_eq!(submit_ticket, ticket);
                coordinator.activate(ticket, JOB_A).unwrap();
                assert_eq!(
                    coordinator.record_result(
                        JOB_A,
                        "sync",
                        PathEditorLifecycleStatus::NativeAccept
                    ),
                    PathEditorResultOwnership::Current
                );
                assert!(matches!(
                    coordinator.begin_reset(identity(DIALOG_A)),
                    PathEditorResetStart::DeferredForSubmit
                ));
            })
            .unwrap();

        let reset = coordinator.begin_reset(identity(DIALOG_A));
        let PathEditorResetStart::Acquired { guard, .. } = reset else {
            panic!("reset lease not acquired");
        };
        assert!(matches!(
            coordinator.begin_reset(identity(DIALOG_A)),
            PathEditorResetStart::DeferredForReset
        ));
        drop(guard);
        let snapshot = publisher.last.lock().unwrap().unwrap();
        assert_clean(snapshot);
        assert!(!snapshot.pending);
    }

    #[test]
    fn old_result_after_reset_reopen_cannot_clear_new_pending_telemetry() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};
        use std::thread;

        let (coordinator, publisher) = coordinator();
        let first = coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |ticket, coordinator| {
                coordinator.activate(ticket, JOB_A).unwrap();
            })
            .unwrap();
        let recognized = Arc::new(Barrier::new(2));
        let native_read_done = Arc::new(Barrier::new(2));
        let record_release = Arc::new(Barrier::new(2));
        let worker_coordinator = Arc::clone(&coordinator);
        let worker_recognized = Arc::clone(&recognized);
        let worker_native_read = Arc::clone(&native_read_done);
        let worker_release = Arc::clone(&record_release);
        let worker = thread::spawn(move || {
            assert!(worker_coordinator.recognizes_job(JOB_A));
            worker_recognized.wait();
            let simulated_native_result = "old-native-read";
            worker_native_read.wait();
            worker_release.wait();
            assert_eq!(
                worker_coordinator.record_result(
                    JOB_A,
                    simulated_native_result,
                    PathEditorLifecycleStatus::NativeAccept
                ),
                PathEditorResultOwnership::StaleOwned
            );
        });

        recognized.wait();
        native_read_done.wait();
        assert!(matches!(
            coordinator.close_with(identity(DIALOG_A), DIALOG_A, |_| true),
            PathEditorCloseDisposition::Closed { closed: true, .. }
        ));
        let owner = AtomicUsize::new(DIALOG_A);
        assert!(matches!(
            coordinator.publish_owner(true, owner.load(Ordering::SeqCst), DIALOG_B, |new| {
                owner.swap(new, Ordering::SeqCst)
            }),
            PathEditorOwnerPublication::Published {
                result: DIALOG_A,
                cancelled_close: None,
                ..
            }
        ));
        let second = coordinator.request(identity(DIALOG_B), DIALOG_B).unwrap();
        assert!(second.generation > first.generation);
        record_release.wait();
        worker.join().unwrap();

        let snapshot = publisher.last.lock().unwrap().unwrap();
        assert!(snapshot.pending);
        assert_eq!(snapshot.generation, second.generation);
        assert_eq!(snapshot.status, PathEditorLifecycleStatus::StaleResult);
        assert_clean(snapshot);
    }

    #[test]
    fn production_tick_gate_blocks_d2_open_rebuild_and_resubmit_until_later_no_ticket_tick() {
        use std::cell::RefCell;

        let calls = RefCell::new(Vec::new());
        for blocked in [
            PathEditorCloseRetryGate::Deferred,
            PathEditorCloseRetryGate::Drained,
        ] {
            assert_eq!(
                run_picker_tick_after_close_gate(
                    || blocked,
                    || calls.borrow_mut().push("open-d2"),
                    || calls.borrow_mut().push("pump"),
                    || calls.borrow_mut().push("rebuild"),
                    || calls.borrow_mut().push("resubmit"),
                ),
                blocked
            );
            assert!(calls.borrow().is_empty());
        }
        assert_eq!(
            run_picker_tick_after_close_gate(
                || PathEditorCloseRetryGate::NoTicket,
                || calls.borrow_mut().push("open-d2"),
                || calls.borrow_mut().push("pump"),
                || calls.borrow_mut().push("rebuild"),
                || calls.borrow_mut().push("resubmit"),
            ),
            PathEditorCloseRetryGate::NoTicket
        );
        assert_eq!(*calls.borrow(), ["open-d2", "pump", "rebuild", "resubmit"]);
    }

    #[test]
    fn picked_file_close_runs_once_before_mode_clear_and_reentrant_resubmit_is_gated() {
        use std::cell::{Cell, RefCell};

        let mode_active = Cell::new(true);
        let open_slots = Cell::new(false);
        let close_sink_calls = Cell::new(0);
        let d2_open_calls = Cell::new(0);
        let order = RefCell::new(Vec::new());

        assert_eq!(
            run_picked_file_close_route(
                || {
                    open_slots.set(true);
                    order.borrow_mut().push("arm-open-slots");
                },
                || {
                    assert!(mode_active.get());
                    assert!(open_slots.get());
                    close_sink_calls.set(close_sink_calls.get() + 1);
                    order.borrow_mut().push("native-close-d1");
                    assert_eq!(
                        run_picker_tick_after_close_gate(
                            || PathEditorCloseRetryGate::Deferred,
                            || d2_open_calls.set(d2_open_calls.get() + 1),
                            || {},
                            || {},
                            || d2_open_calls.set(d2_open_calls.get() + 1),
                        ),
                        PathEditorCloseRetryGate::Deferred
                    );
                    true
                },
                || {
                    mode_active.set(false);
                    order.borrow_mut().push("clear-picker-mode");
                },
            ),
            PickedFileCloseRoute::ClosedAndCleared
        );
        assert_eq!(close_sink_calls.get(), 1);
        assert_eq!(d2_open_calls.get(), 0);
        assert!(!mode_active.get());
        assert_eq!(
            *order.borrow(),
            ["arm-open-slots", "native-close-d1", "clear-picker-mode"]
        );
    }

    #[test]
    fn deferred_picked_file_sequence_waits_through_retry_and_drain_before_resubmit() {
        use std::cell::{Cell, RefCell};

        let mode_active = Cell::new(true);
        let open_slots = Cell::new(false);
        let calls = RefCell::new(Vec::new());
        assert_eq!(
            run_picked_file_close_route(
                || open_slots.set(true),
                || {
                    calls.borrow_mut().push("request-close-d1");
                    false
                },
                || panic!("deferred close cleared picker mode"),
            ),
            PickedFileCloseRoute::Deferred
        );
        assert!(mode_active.get());
        assert!(open_slots.get());

        for gate in [
            PathEditorCloseRetryGate::Deferred,
            PathEditorCloseRetryGate::Drained,
        ] {
            assert_eq!(
                run_picker_tick_after_close_gate(
                    || {
                        calls.borrow_mut().push(match gate {
                            PathEditorCloseRetryGate::Deferred => "retry-deferred",
                            PathEditorCloseRetryGate::Drained => "retry-drained",
                            PathEditorCloseRetryGate::NoTicket => unreachable!(),
                        });
                        gate
                    },
                    || calls.borrow_mut().push("open-d2"),
                    || calls.borrow_mut().push("pump"),
                    || calls.borrow_mut().push("rebuild"),
                    || calls.borrow_mut().push("resubmit"),
                ),
                gate
            );
        }
        mode_active.set(false);
        calls.borrow_mut().push("clear-picker-mode-after-drain");
        assert_eq!(
            run_picker_tick_after_close_gate(
                || PathEditorCloseRetryGate::NoTicket,
                || calls.borrow_mut().push("open-d2"),
                || calls.borrow_mut().push("pump"),
                || calls.borrow_mut().push("rebuild"),
                || calls.borrow_mut().push("resubmit"),
            ),
            PathEditorCloseRetryGate::NoTicket
        );
        assert_eq!(
            *calls.borrow(),
            [
                "request-close-d1",
                "retry-deferred",
                "retry-drained",
                "clear-picker-mode-after-drain",
                "open-d2",
                "pump",
                "rebuild",
                "resubmit",
            ]
        );
    }

    #[test]
    fn completed_exact_generation_can_cross_native_owner_zero_for_no_close_return() {
        let (coordinator, _) = coordinator();
        coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |ticket, coordinator| {
                coordinator.activate(ticket, JOB_A).unwrap();
            })
            .unwrap();
        assert_eq!(
            coordinator.record_result(
                JOB_A,
                "accepted after parent finish",
                PathEditorLifecycleStatus::NativeAccept,
            ),
            PathEditorResultOwnership::Current
        );
        let owner_zero = PathEditorPickerIdentity {
            picker_mode_active: true,
            current_dialog: 0,
        };
        let completed = coordinator
            .take_completed_for_owner_transition(owner_zero)
            .unwrap()
            .unwrap();
        assert_eq!(completed.0.dialog, DIALOG_A);
        assert_eq!(completed.1, "accepted after parent finish");
        assert_eq!(coordinator.take_completed(identity(DIALOG_A)), Ok(None));
    }

    #[test]
    fn compare_owner_publication_cannot_clear_a_newer_native_owner() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (coordinator, _) = coordinator();
        coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        let native_owner = AtomicUsize::new(DIALOG_B);
        assert!(matches!(
            coordinator.publish_owner_if_current(true, DIALOG_A, 0, |expected, owner| native_owner
                .compare_exchange(expected, owner, Ordering::SeqCst, Ordering::SeqCst,),),
            PathEditorOwnerComparePublication::Stale { actual: DIALOG_B }
        ));
        assert_eq!(native_owner.load(Ordering::SeqCst), DIALOG_B);
        assert_eq!(coordinator.snapshot().owner_dialog, DIALOG_A);
        assert!(coordinator.snapshot().pending);

        native_owner.store(DIALOG_A, Ordering::SeqCst);
        assert!(matches!(
            coordinator.publish_owner_if_current(true, DIALOG_A, 0, |expected, owner| native_owner
                .compare_exchange(expected, owner, Ordering::SeqCst, Ordering::SeqCst,),),
            PathEditorOwnerComparePublication::Published {
                result: DIALOG_A,
                ..
            }
        ));
        assert_eq!(native_owner.load(Ordering::SeqCst), 0);
        assert_eq!(coordinator.snapshot().owner_dialog, 0);
        assert!(!coordinator.snapshot().pending);
    }

    #[test]
    fn abort_active_submit_retires_unsubmitted_job_without_terminal_completion() {
        let (coordinator, _) = coordinator();
        let ticket = coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |ticket, coordinator| {
                coordinator.activate(ticket, JOB_A).unwrap();
            })
            .unwrap();
        assert!(coordinator.abort_active_submit(ticket, JOB_A));
        assert_eq!(coordinator.active_job(), None);
        assert!(coordinator.recognizes_job(JOB_A));
        assert_eq!(
            coordinator.record_result(
                JOB_A,
                "late result after aborted native submit",
                PathEditorLifecycleStatus::NativeAccept,
            ),
            PathEditorResultOwnership::StaleOwned
        );
        assert_eq!(coordinator.take_completed(identity(DIALOG_A)), Ok(None));
    }

    #[test]
    fn newer_owner_published_after_old_result_acquisition_rejects_transaction_before_effects() {
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (coordinator, _) = coordinator();
        let ticket = coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |ticket, coordinator| {
                coordinator.activate(ticket, JOB_A).unwrap();
            })
            .unwrap();
        assert_eq!(
            coordinator.record_result(
                JOB_A,
                "old terminal result",
                PathEditorLifecycleStatus::NativeAccept,
            ),
            PathEditorResultOwnership::Current
        );
        let acquired = coordinator
            .take_completed_for_owner_transition(identity(DIALOG_A))
            .unwrap()
            .unwrap();
        assert_eq!(acquired.0, ticket);

        let acquisition_pause = std::sync::Arc::new(Barrier::new(2));
        let publication_done = std::sync::Arc::new(Barrier::new(2));
        let worker = {
            let coordinator = coordinator.clone();
            let acquisition_pause = acquisition_pause.clone();
            let publication_done = publication_done.clone();
            std::thread::spawn(move || {
                acquisition_pause.wait();
                let _ = coordinator.publish_owner(true, DIALOG_A, DIALOG_B, |_| DIALOG_A);
                coordinator.request(identity(DIALOG_B), DIALOG_B).unwrap();
                publication_done.wait();
            })
        };
        acquisition_pause.wait();
        publication_done.wait();
        worker.join().unwrap();
        assert_eq!(coordinator.snapshot().owner_dialog, DIALOG_B);

        let model_mutations = AtomicUsize::new(0);
        let reopens = AtomicUsize::new(0);
        let refreshes = AtomicUsize::new(0);
        assert!(matches!(
            coordinator.with_terminal_result_transaction(identity(DIALOG_B), acquired.0, || {
                model_mutations.fetch_add(1, Ordering::SeqCst);
                reopens.fetch_add(1, Ordering::SeqCst);
                refreshes.fetch_add(1, Ordering::SeqCst);
            },),
            Err(PathEditorLifecycleRejection::RequestMismatch)
        ));
        assert_eq!(model_mutations.load(Ordering::SeqCst), 0);
        assert_eq!(reopens.load(Ordering::SeqCst), 0);
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn same_address_new_generation_rejects_acquired_old_result_transaction() {
        let (coordinator, _) = coordinator();
        let ticket = coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        coordinator
            .with_submit(identity(DIALOG_A), |ticket, coordinator| {
                coordinator.activate(ticket, JOB_A).unwrap();
            })
            .unwrap();
        assert_eq!(
            coordinator.record_result(
                JOB_A,
                "old terminal result",
                PathEditorLifecycleStatus::NativeAccept,
            ),
            PathEditorResultOwnership::Current
        );
        let acquired = coordinator
            .take_completed_for_owner_transition(identity(DIALOG_A))
            .unwrap()
            .unwrap();
        assert_eq!(acquired.0, ticket);
        let _ = coordinator.publish_owner(true, DIALOG_A, 0, |_| DIALOG_A);
        let _ = coordinator.publish_owner(true, 0, DIALOG_A, |_| 0);
        coordinator.request(identity(DIALOG_A), DIALOG_A).unwrap();
        assert!(coordinator.snapshot().generation > ticket.generation);
        assert_eq!(coordinator.snapshot().owner_dialog, DIALOG_A);
        assert!(matches!(
            coordinator.with_terminal_result_transaction(identity(DIALOG_A), acquired.0, || {
                panic!("ABA result must reject before retained-model mutation or return arm")
            }),
            Err(PathEditorLifecycleRejection::RequestMismatch)
        ));
    }
}
