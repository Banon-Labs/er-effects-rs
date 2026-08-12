use crate::DriveStripFocus;

/// One immutable transform captured from the validated game HWND. Event-local points are already
/// client-local; independently sampled live-pointer points are screen-local. Both routes terminate
/// in the same fitted movie-stage transform so a physical click can compare like with like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriveStripMovieViewport {
    pub client_origin_screen_x: f32,
    pub client_origin_screen_y: f32,
    pub client_width: f32,
    pub client_height: f32,
    pub movie_width: f32,
    pub movie_height: f32,
}

impl DriveStripMovieViewport {
    fn is_valid(self) -> bool {
        self.client_origin_screen_x.is_finite()
            && self.client_origin_screen_y.is_finite()
            && self.client_width.is_finite()
            && self.client_height.is_finite()
            && self.movie_width.is_finite()
            && self.movie_height.is_finite()
            && self.client_width > 0.0
            && self.client_height > 0.0
            && self.movie_width > 0.0
            && self.movie_height > 0.0
    }

    /// Convert one point already expressed relative to the HWND client origin into the authored
    /// movie stage. Points outside either the client or the aspect-fitted viewport fail closed.
    pub fn client_point_to_movie_stage(self, client_x: f32, client_y: f32) -> Option<(f32, f32)> {
        if !self.is_valid()
            || !client_x.is_finite()
            || !client_y.is_finite()
            || client_x < 0.0
            || client_y < 0.0
            || client_x >= self.client_width
            || client_y >= self.client_height
        {
            return None;
        }

        let movie_aspect = self.movie_width / self.movie_height;
        let client_aspect = self.client_width / self.client_height;
        let (content_x, content_y, content_width, content_height) = if client_aspect > movie_aspect
        {
            let content_width = self.client_height * movie_aspect;
            (
                (self.client_width - content_width) * 0.5,
                0.0,
                content_width,
                self.client_height,
            )
        } else {
            let content_height = self.client_width / movie_aspect;
            (
                0.0,
                (self.client_height - content_height) * 0.5,
                self.client_width,
                content_height,
            )
        };
        let viewport_x = client_x - content_x;
        let viewport_y = client_y - content_y;
        if viewport_x < 0.0
            || viewport_y < 0.0
            || viewport_x >= content_width
            || viewport_y >= content_height
        {
            return None;
        }
        Some((
            (viewport_x / content_width) * self.movie_width - self.movie_width * 0.5,
            (viewport_y / content_height) * self.movie_height - self.movie_height * 0.5,
        ))
    }

    /// Convert one OS screen point through this exact HWND's captured client origin and viewport.
    pub fn screen_point_to_movie_stage(self, screen_x: f32, screen_y: f32) -> Option<(f32, f32)> {
        if !screen_x.is_finite() || !screen_y.is_finite() {
            return None;
        }
        self.client_point_to_movie_stage(
            screen_x - self.client_origin_screen_x,
            screen_y - self.client_origin_screen_y,
        )
    }

    /// Return the client-local integer position used only for stationary-pointer ownership.
    pub fn screen_point_to_client(self, screen_x: f32, screen_y: f32) -> Option<(u32, u32)> {
        let client_x = screen_x - self.client_origin_screen_x;
        let client_y = screen_y - self.client_origin_screen_y;
        self.client_point_to_movie_stage(client_x, client_y)?;
        Some((client_x as u32, client_y as u32))
    }
}

/// One activation resolved from the drive/path row's explicit subfocus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripActivation {
    SelectCell(usize),
    OpenCurrentPath,
}

impl From<DriveStripFocus> for DriveStripActivation {
    fn from(focus: DriveStripFocus) -> Self {
        match focus {
            DriveStripFocus::Cell(cell) => Self::SelectCell(cell),
            DriveStripFocus::CurrentPath => Self::OpenCurrentPath,
        }
    }
}

/// Movie-stage hit bounds shared by hover and physical-click routing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriveStripPointerBounds {
    pub first_cell_left: f32,
    pub cell_pitch: f32,
    pub cell_width: f32,
    pub path_left: f32,
    pub path_width: f32,
    pub row_top: f32,
    pub row_height: f32,
}

impl DriveStripPointerBounds {
    /// Classify only points inside the drive row's vertical band. Half-open bounds make adjacent
    /// controls deterministic and reject the exact right/bottom edge.
    pub fn classify(self, x: f32, y: f32, cell_count: usize) -> Option<DriveStripFocus> {
        if cell_count == 0
            || !x.is_finite()
            || !y.is_finite()
            || self.cell_pitch <= 0.0
            || self.cell_width <= 0.0
            || self.path_width <= 0.0
            || self.row_height <= 0.0
            || !(self.row_top..self.row_top + self.row_height).contains(&y)
        {
            return None;
        }

        let local_x = x - self.first_cell_left;
        if local_x >= 0.0 {
            let cell = (local_x / self.cell_pitch).floor() as usize;
            if cell < cell_count {
                let in_cell_x = local_x - cell as f32 * self.cell_pitch;
                if in_cell_x < self.cell_width {
                    return Some(DriveStripFocus::Cell(cell));
                }
            }
        }

        (self.path_left..self.path_left + self.path_width)
            .contains(&x)
            .then_some(DriveStripFocus::CurrentPath)
    }
}

/// Win32 facts are collected by the unsafe adapter. Keeping their validity decision pure makes
/// foreign/background/invalid-client rejection red-capable without pretending host tests can own
/// an Elden Ring HWND.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriveStripWindowFacts {
    pub hwnd_present: bool,
    pub foreground_matches: bool,
    pub same_process: bool,
    pub client_geometry_valid: bool,
    pub pointer_in_client: bool,
}

impl DriveStripWindowFacts {
    pub const fn accepts_pointer(self) -> bool {
        self.hwnd_present
            && self.foreground_matches
            && self.same_process
            && self.client_geometry_valid
            && self.pointer_in_client
    }
}

/// Pure input to the pointer-move router. `last_pointer_position` is observational state only; the
/// adapter commits the returned position after the requested native row-focus transition succeeds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriveStripPointerFacts {
    pub window: DriveStripWindowFacts,
    pub native_row: usize,
    pub drive_row: usize,
    pub controls_visible: bool,
    pub cell_count: usize,
    pub pointer_position: u64,
    pub last_pointer_position: Option<u64>,
    pub stage_x: f32,
    pub stage_y: f32,
    pub bounds: DriveStripPointerBounds,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriveStripPointerDecision {
    pub target: DriveStripFocus,
    pub native_row_focus: Option<usize>,
    pub commit_pointer_position: u64,
    pub rebuild_cursor: bool,
}

/// One already-validated physical pointer sample supplied to the pure menu-pump seam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriveStripPointerSample {
    pub window: DriveStripWindowFacts,
    pub packed_position: u64,
    pub stage_x: f32,
    pub stage_y: f32,
}

/// Pure result of one drive-strip menu-pump iteration. Native staging and pointer publication stay
/// with the runtime adapter, but its device-ownership ordering is decided here and host-tested.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriveStripPumpPlan {
    pub native_row_changed: bool,
    pub keyboard_navigation: bool,
    pub keyboard_snapshot: Option<crate::DriveStripInteractionState>,
    pub presentation_needs_stage: bool,
    /// Whether the staged presentation change is one only a fresh owner can carry. Pointer hover
    /// never sets this; a fresh-owner refresh closes the live window, so hover must not request one.
    pub presentation_requires_fresh_owner: bool,
    pub pointer_absent: bool,
    pub pointer_decision: Option<DriveStripPointerDecision>,
}

/// Observe native keyboard/pad ownership, optionally apply horizontal keyboard navigation, then
/// route the current physical pointer sample. A valid stationary sample remains physical presence
/// but produces no pointer decision. Invalid, out-of-band, or hidden-control samples are genuine
/// absence and clear the committed position through `pointer_left`.
pub fn orchestrate_drive_strip_pump(
    model: &mut crate::SavePickerModel,
    native_row: usize,
    controls_visible: bool,
    keyboard_move_forward: Option<bool>,
    pointer: Option<DriveStripPointerSample>,
    bounds: DriveStripPointerBounds,
) -> Option<DriveStripPumpPlan> {
    let drive_row = model.drive_row()?;
    let cell_count = model.drive_strip_cell_count();
    let native_row_changed = model.observe_drive_strip_native_row(native_row);

    if native_row == drive_row {
        if let Some(forward) = keyboard_move_forward {
            let snapshot = model.drive_strip_interaction_snapshot();
            let _ = model.move_drive_strip_focus(forward);
            return Some(DriveStripPumpPlan {
                native_row_changed,
                keyboard_navigation: true,
                keyboard_snapshot: Some(snapshot),
                presentation_needs_stage: model.drive_strip_presentation_dirty(),
                presentation_requires_fresh_owner: model
                    .drive_strip_presentation_requires_fresh_owner(),
                pointer_absent: false,
                pointer_decision: None,
            });
        }
    }

    let pointer_in_control = pointer.filter(|sample| {
        sample.window.accepts_pointer()
            && controls_visible
            && bounds
                .classify(sample.stage_x, sample.stage_y, cell_count)
                .is_some()
    });
    let pointer_decision = if let Some(sample) = pointer_in_control {
        route_drive_strip_pointer_move(DriveStripPointerFacts {
            window: sample.window,
            native_row,
            drive_row,
            controls_visible,
            cell_count,
            pointer_position: sample.packed_position,
            last_pointer_position: model.drive_strip_pointer_position(),
            stage_x: sample.stage_x,
            stage_y: sample.stage_y,
            bounds,
        })
    } else {
        let _ = model.drive_strip_pointer_left();
        None
    };

    Some(DriveStripPumpPlan {
        native_row_changed,
        keyboard_navigation: false,
        keyboard_snapshot: None,
        presentation_needs_stage: model.drive_strip_presentation_dirty(),
        presentation_requires_fresh_owner: model.drive_strip_presentation_requires_fresh_owner(),
        pointer_absent: pointer_in_control.is_none(),
        pointer_decision,
    })
}

/// Resolve eligibility and target identity before allowing the pointer position to be committed.
pub fn route_drive_strip_pointer_move(
    facts: DriveStripPointerFacts,
) -> Option<DriveStripPointerDecision> {
    if !facts.window.accepts_pointer()
        || !facts.controls_visible
        || facts.cell_count == 0
        || facts.last_pointer_position == Some(facts.pointer_position)
    {
        return None;
    }
    let target = facts
        .bounds
        .classify(facts.stage_x, facts.stage_y, facts.cell_count)?;
    Some(DriveStripPointerDecision {
        target,
        native_row_focus: (facts.native_row != facts.drive_row).then_some(facts.drive_row),
        commit_pointer_position: facts.pointer_position,
        rebuild_cursor: true,
    })
}

/// Ordered gate for pointer hover publication. The legacy step names remain API-compatible, but
/// production now uses them for presentation preparation and a fresh-owner refresh request; it does
/// not stage ProfileSummary while the old owner is live. Same-target motion skips both steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripPointerTransactionStep {
    NativeFocus,
    ModelFocus,
    RowRecordsStaged,
    RebuildScheduled,
    Committed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriveStripPointerTransaction {
    step: DriveStripPointerTransactionStep,
}

impl DriveStripPointerTransaction {
    pub const fn new() -> Self {
        Self {
            step: DriveStripPointerTransactionStep::NativeFocus,
        }
    }

    pub const fn step(self) -> DriveStripPointerTransactionStep {
        self.step
    }

    pub fn native_focus_result(&mut self, succeeded: bool) -> bool {
        self.advance(
            DriveStripPointerTransactionStep::NativeFocus,
            DriveStripPointerTransactionStep::ModelFocus,
            succeeded,
        )
    }

    pub fn model_focus_result(&mut self, succeeded: bool) -> bool {
        self.advance(
            DriveStripPointerTransactionStep::ModelFocus,
            DriveStripPointerTransactionStep::RowRecordsStaged,
            succeeded,
        )
    }

    pub fn row_staging_result(&mut self, succeeded: bool) -> bool {
        self.advance(
            DriveStripPointerTransactionStep::RowRecordsStaged,
            DriveStripPointerTransactionStep::RebuildScheduled,
            succeeded,
        )
    }

    /// Same-target physical motion changes only the committed position. No presentation work was
    /// provisioned, so neither the prepare nor close/resubmit sink may run.
    pub fn skip_unchanged_presentation(&mut self) -> bool {
        self.advance(
            DriveStripPointerTransactionStep::RowRecordsStaged,
            DriveStripPointerTransactionStep::Committed,
            true,
        )
    }

    pub fn rebuild_scheduling_result(&mut self, succeeded: bool) -> bool {
        self.advance(
            DriveStripPointerTransactionStep::RebuildScheduled,
            DriveStripPointerTransactionStep::Committed,
            succeeded,
        )
    }

    pub const fn can_commit_pointer(self) -> bool {
        matches!(self.step, DriveStripPointerTransactionStep::Committed)
    }

    pub const fn rollback_required(self) -> bool {
        matches!(self.step, DriveStripPointerTransactionStep::Failed)
    }

    fn advance(
        &mut self,
        expected: DriveStripPointerTransactionStep,
        next: DriveStripPointerTransactionStep,
        succeeded: bool,
    ) -> bool {
        if self.step != expected {
            self.step = DriveStripPointerTransactionStep::Failed;
            return false;
        }
        self.step = if succeeded {
            next
        } else {
            DriveStripPointerTransactionStep::Failed
        };
        succeeded
    }
}

impl Default for DriveStripPointerTransaction {
    fn default() -> Self {
        Self::new()
    }
}

/// Why pointer publication failed after its plan was accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripPointerTransactionFailure {
    NativeFocus,
    MissingModel,
    InvalidProvisionalFocus,
    RowRecordsStaged,
    RebuildScheduled,
    MissingModelBeforeCommit,
}

/// Observable result of restoring both owners of the provisional pointer publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriveStripPointerRollbackResult {
    pub model_restored: bool,
    pub row_records_restaged: bool,
    pub native_row_restored: bool,
}

/// Final result from the transaction executor used by the runtime adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripPointerTransactionOutcome {
    Committed,
    NativeFocusRejected,
    RolledBack {
        failure: DriveStripPointerTransactionFailure,
        rollback: DriveStripPointerRollbackResult,
    },
}

/// Result of acquiring and provisionally updating the live model after native focus publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripPointerProvisionFailure {
    MissingModel,
    InvalidProvisionalFocus,
}

/// Publish pointer ownership as one rollback-complete transaction. Provision returns whether the
/// visible target/owner changed. Same-target motion commits only position and calls neither
/// presentation sink; every failure after native focus uses the same model/native rollback.
pub fn execute_drive_strip_pointer_transaction(
    publish_native_focus: impl FnOnce() -> bool,
    provision_model_focus: impl FnOnce() -> Result<bool, DriveStripPointerProvisionFailure>,
    prepare_presentation: impl FnOnce() -> bool,
    schedule_refresh: impl FnOnce() -> bool,
    commit_pointer: impl FnOnce() -> bool,
    mut rollback: impl FnMut() -> DriveStripPointerRollbackResult,
) -> DriveStripPointerTransactionOutcome {
    let mut transaction = DriveStripPointerTransaction::new();
    if !transaction.native_focus_result(publish_native_focus()) {
        return DriveStripPointerTransactionOutcome::NativeFocusRejected;
    }

    let provision = provision_model_focus();
    if !transaction.model_focus_result(provision.is_ok()) {
        let failure = match provision {
            Err(DriveStripPointerProvisionFailure::MissingModel) => {
                DriveStripPointerTransactionFailure::MissingModel
            }
            Err(DriveStripPointerProvisionFailure::InvalidProvisionalFocus) => {
                DriveStripPointerTransactionFailure::InvalidProvisionalFocus
            }
            Ok(_) => unreachable!("successful provision cannot fail its transaction gate"),
        };
        return DriveStripPointerTransactionOutcome::RolledBack {
            failure,
            rollback: rollback(),
        };
    }
    if provision == Ok(false) {
        let skipped = transaction.skip_unchanged_presentation();
        debug_assert!(skipped && transaction.can_commit_pointer());
    } else {
        if !transaction.row_staging_result(prepare_presentation()) {
            return DriveStripPointerTransactionOutcome::RolledBack {
                failure: DriveStripPointerTransactionFailure::RowRecordsStaged,
                rollback: rollback(),
            };
        }
        if !transaction.rebuild_scheduling_result(schedule_refresh()) {
            return DriveStripPointerTransactionOutcome::RolledBack {
                failure: DriveStripPointerTransactionFailure::RebuildScheduled,
                rollback: rollback(),
            };
        }
    }
    debug_assert!(transaction.can_commit_pointer());
    if !commit_pointer() {
        return DriveStripPointerTransactionOutcome::RolledBack {
            failure: DriveStripPointerTransactionFailure::MissingModelBeforeCommit,
            rollback: rollback(),
        };
    }
    DriveStripPointerTransactionOutcome::Committed
}

/// Provenance of one native picker activation. The four known variants remain non-aliasing;
/// `UnknownNativeActivation` is fail-closed state and can never stand in for keyboard/pad Accept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripActivationProvenance {
    KeyboardOrPadAccept,
    AcceptedPhysicalClick(DriveStripFocus),
    RejectedPhysicalClick,
    OrdinaryRowPhysicalActivation,
    UnknownNativeActivation,
}

impl DriveStripActivationProvenance {
    pub const fn physical_click(target: Option<DriveStripFocus>) -> Self {
        match target {
            Some(target) => Self::AcceptedPhysicalClick(target),
            None => Self::RejectedPhysicalClick,
        }
    }

    pub const fn is_physical(self) -> bool {
        matches!(
            self,
            Self::AcceptedPhysicalClick(_)
                | Self::RejectedPhysicalClick
                | Self::OrdinaryRowPhysicalActivation
        )
    }
}

/// Legacy pure-test helper for replaying the discredited pending-latch lifecycle. Production does
/// not call this: ProfileLoad provenance is owned by the scoped MenuWindow update context.
pub fn forward_drive_strip_native_activation_once(
    provenance: DriveStripActivationProvenance,
    mut arm: impl FnMut(DriveStripActivationProvenance),
    forward_native: impl FnOnce(),
    mut clear_after_forward: impl FnMut(),
) {
    arm(provenance);
    forward_native();
    clear_after_forward();
}

/// Resolve the row-level part of a physical picker activation before drive-strip hit testing.
/// The live ProfileLoad cursor is already the model row; a non-drive row keeps native/model
/// activation semantics and must not enter the drive-strip rejection path.
pub fn classify_picker_physical_row(
    model_row: usize,
    drive_row: Option<usize>,
) -> Option<DriveStripActivationProvenance> {
    if drive_row == Some(model_row) {
        None
    } else {
        Some(DriveStripActivationProvenance::OrdinaryRowPhysicalActivation)
    }
}

/// Resolve a native physical click through the same X/Y classifier used by hover. The native event
/// hook owns the click transaction; the menu pump never emits an activation.
pub fn route_drive_strip_native_click(
    window: DriveStripWindowFacts,
    native_row: usize,
    drive_row: usize,
    controls_visible: bool,
    cell_count: usize,
    stage_x: f32,
    stage_y: f32,
    bounds: DriveStripPointerBounds,
) -> Option<DriveStripFocus> {
    if !window.accepts_pointer() || native_row != drive_row || !controls_visible {
        return None;
    }
    bounds.classify(stage_x, stage_y, cell_count)
}

/// Require the event-local and independently sampled live pointer to name the same control.
pub fn agree_drive_strip_click_targets(
    event_target: Option<DriveStripFocus>,
    live_target: Option<DriveStripFocus>,
) -> Option<DriveStripFocus> {
    event_target.filter(|target| Some(*target) == live_target)
}

/// Product effect of one drive/path activation after pending-click-or-keyboard resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripActivationEffect {
    DriveSelected(usize),
    RequestPathEditor,
    Ignored,
}

/// Product effect of one native picker activation. Ordinary rows retain the model's native row
/// semantics; drive/path clicks remain explicitly separated from keyboard/pad Accept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PickerNativeActivationEffect {
    Model(crate::PickerActivation),
    DriveSelected(usize),
    RequestPathEditor,
    Ignored,
}

/// Why a native-accepted picker callback produced no model/editor effect. These are terminal,
/// named fail-closed outcomes; `UnknownNativeActivation` is never reinterpreted as Accept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerSourceRejection {
    MissingModel,
    InvalidModelRow,
    UnknownSource,
    RejectedPhysicalClick,
    CrossRowProvenance,
    StatusOwnedRow,
    DuplicateCallback,
    LateCallback,
    ModelIgnored,
}

/// Decision at the production ProfileLoad adapter seam. Only non-picker or identity-mismatched
/// dispatches may forward native behavior. Every picker callback terminates as one effect or one
/// named rejection, and therefore never queues the native ProfileLoad job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PickerSourceDecision {
    ForwardNative,
    Effect(PickerNativeActivationEffect),
    Rejected(PickerSourceRejection),
}

/// Pure counterpart of the runtime adapter used by the scoped MenuWindow/ProfileLoad hooks.
/// `dialog_matches` is the runtime's exact active-dialog + vtable identity result.
pub fn reject_picker_late_callback() -> PickerSourceDecision {
    PickerSourceDecision::Rejected(PickerSourceRejection::LateCallback)
}

pub fn route_picker_source_activation(
    picker_mode: bool,
    dialog_matches: bool,
    model: Option<&mut crate::SavePickerModel>,
    model_row: Option<usize>,
    provenance: DriveStripActivationProvenance,
) -> PickerSourceDecision {
    if !picker_mode || !dialog_matches {
        return PickerSourceDecision::ForwardNative;
    }
    let Some(model) = model else {
        return PickerSourceDecision::Rejected(PickerSourceRejection::MissingModel);
    };
    let Some(model_row) = model_row.filter(|row| *row < model.visible_row_count()) else {
        return PickerSourceDecision::Rejected(PickerSourceRejection::InvalidModelRow);
    };
    let is_drive_row = model.drive_row() == Some(model_row);
    let rejection = match provenance {
        DriveStripActivationProvenance::UnknownNativeActivation => {
            Some(PickerSourceRejection::UnknownSource)
        }
        DriveStripActivationProvenance::RejectedPhysicalClick => {
            Some(PickerSourceRejection::RejectedPhysicalClick)
        }
        DriveStripActivationProvenance::OrdinaryRowPhysicalActivation if is_drive_row => {
            Some(PickerSourceRejection::CrossRowProvenance)
        }
        DriveStripActivationProvenance::AcceptedPhysicalClick(_) if !is_drive_row => {
            Some(PickerSourceRejection::CrossRowProvenance)
        }
        _ if is_drive_row && model.status_message().is_some() => {
            Some(PickerSourceRejection::StatusOwnedRow)
        }
        _ => None,
    };
    if let Some(rejection) = rejection {
        return PickerSourceDecision::Rejected(rejection);
    }
    let effect = orchestrate_picker_native_activation(model, model_row, provenance);
    if matches!(effect, PickerNativeActivationEffect::Ignored) {
        PickerSourceDecision::Rejected(PickerSourceRejection::ModelIgnored)
    } else {
        PickerSourceDecision::Effect(effect)
    }
}

/// Pure production orchestration shared by the ProfileLoad activation handler and route tests.
/// Cross-row provenance is fail-closed: drive-strip provenance cannot activate an ordinary row,
/// and ordinary-row physical provenance cannot be reinterpreted as drive-strip input.
pub fn orchestrate_picker_native_activation(
    model: &mut crate::SavePickerModel,
    model_row: usize,
    provenance: DriveStripActivationProvenance,
) -> PickerNativeActivationEffect {
    let is_drive_row = model.drive_row() == Some(model_row);
    if !is_drive_row {
        return match provenance {
            DriveStripActivationProvenance::KeyboardOrPadAccept
            | DriveStripActivationProvenance::OrdinaryRowPhysicalActivation => {
                PickerNativeActivationEffect::Model(model.activate(model_row))
            }
            DriveStripActivationProvenance::AcceptedPhysicalClick(_)
            | DriveStripActivationProvenance::RejectedPhysicalClick
            | DriveStripActivationProvenance::UnknownNativeActivation => {
                PickerNativeActivationEffect::Ignored
            }
        };
    }
    if matches!(
        provenance,
        DriveStripActivationProvenance::OrdinaryRowPhysicalActivation
            | DriveStripActivationProvenance::UnknownNativeActivation
    ) || model.status_message().is_some()
    {
        return PickerNativeActivationEffect::Ignored;
    }
    match orchestrate_drive_strip_activation(model, provenance) {
        DriveStripActivationEffect::DriveSelected(cell) => {
            PickerNativeActivationEffect::DriveSelected(cell)
        }
        DriveStripActivationEffect::RequestPathEditor => {
            PickerNativeActivationEffect::RequestPathEditor
        }
        DriveStripActivationEffect::Ignored => PickerNativeActivationEffect::Ignored,
    }
}

/// Pure production orchestration for the drive/path row. This is the only seam that turns a
/// resolved target into model mutation or a path-editor request.
pub fn orchestrate_drive_strip_activation(
    model: &mut crate::SavePickerModel,
    provenance: DriveStripActivationProvenance,
) -> DriveStripActivationEffect {
    match resolve_drive_strip_activation(provenance, model.drive_strip_focus()) {
        Some(DriveStripActivation::OpenCurrentPath) => {
            DriveStripActivationEffect::RequestPathEditor
        }
        Some(DriveStripActivation::SelectCell(cell)) if model.activate_drive_strip_cell(cell) => {
            DriveStripActivationEffect::DriveSelected(cell)
        }
        _ => DriveStripActivationEffect::Ignored,
    }
}

/// Resolve accepted physical clicks only to their event target, rejected physical clicks to no
/// action, and keyboard/pad Accept only to the explicit keyboard subfocus.
pub fn resolve_drive_strip_activation(
    provenance: DriveStripActivationProvenance,
    explicit_focus: Option<DriveStripFocus>,
) -> Option<DriveStripActivation> {
    match provenance {
        DriveStripActivationProvenance::KeyboardOrPadAccept => explicit_focus.map(Into::into),
        DriveStripActivationProvenance::AcceptedPhysicalClick(target) => Some(target.into()),
        DriveStripActivationProvenance::RejectedPhysicalClick
        | DriveStripActivationProvenance::OrdinaryRowPhysicalActivation
        | DriveStripActivationProvenance::UnknownNativeActivation => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: DriveStripPointerBounds = DriveStripPointerBounds {
        first_cell_left: -422.0,
        cell_pitch: 32.0,
        cell_width: 32.0,
        path_left: -182.0,
        path_width: 600.0,
        row_top: -236.0,
        row_height: 39.0,
    };
    const VALID_WINDOW: DriveStripWindowFacts = DriveStripWindowFacts {
        hwnd_present: true,
        foreground_matches: true,
        same_process: true,
        client_geometry_valid: true,
        pointer_in_client: true,
    };

    fn pointer_facts() -> DriveStripPointerFacts {
        DriveStripPointerFacts {
            window: VALID_WINDOW,
            native_row: 3,
            drive_row: 0,
            controls_visible: true,
            cell_count: 3,
            pointer_position: 0x1234,
            last_pointer_position: None,
            stage_x: -389.0,
            stage_y: -210.0,
            bounds: BOUNDS,
        }
    }

    #[test]
    fn row_three_pointer_routes_to_native_drive_row_before_position_commit() {
        let decision = route_drive_strip_pointer_move(pointer_facts()).expect("eligible movement");
        assert_eq!(decision.target, DriveStripFocus::Cell(1));
        assert_eq!(decision.native_row_focus, Some(0));
        assert_eq!(decision.commit_pointer_position, 0x1234);
        assert!(decision.rebuild_cursor);
    }

    #[test]
    fn rejected_row_movement_is_not_consumed_before_focus_can_change() {
        let mut facts = pointer_facts();
        facts.controls_visible = false;
        assert_eq!(route_drive_strip_pointer_move(facts), None);
        facts.controls_visible = true;
        assert!(route_drive_strip_pointer_move(facts).is_some());
    }

    #[test]
    fn same_pixel_leave_and_reenter_routes_again() {
        let mut state = crate::DriveStripInteractionState::default();
        state.observe_native_row(3, 0);
        let mut facts = pointer_facts();
        facts.last_pointer_position = state.committed_pointer_position();
        let first = route_drive_strip_pointer_move(facts).expect("first entry");
        let snapshot = state.provision_pointer_hover(first.target, 0);
        assert_ne!(snapshot, state);
        state.commit_pointer_position(first.commit_pointer_position);
        assert_eq!(
            state.committed_pointer_position(),
            Some(facts.pointer_position)
        );

        state.pointer_left();
        assert!(
            state.presentation_dirty(),
            "failed leave presentation can retry without restoring pointer presence"
        );
        let mut retry_after_failed_leave = facts;
        retry_after_failed_leave.native_row = 0;
        retry_after_failed_leave.last_pointer_position = state.committed_pointer_position();
        assert!(
            route_drive_strip_pointer_move(retry_after_failed_leave).is_some(),
            "same-pixel return retries even before the dirty leave presentation rebuild succeeds"
        );
        state.observe_native_row(3, 0);
        facts.native_row = 3;
        facts.last_pointer_position = state.committed_pointer_position();
        assert_eq!(facts.last_pointer_position, None);
        let reentry = route_drive_strip_pointer_move(facts).expect("same-pixel re-entry");
        assert_eq!(reentry.native_row_focus, Some(0));
        assert_eq!(reentry.target, first.target);
    }

    #[test]
    fn stage_or_rebuild_failure_does_not_commit_and_rollback_allows_retry() {
        for fail_at_stage in [true, false] {
            let decision = route_drive_strip_pointer_move(pointer_facts()).expect("eligible");
            let mut state = crate::DriveStripInteractionState::default();
            state.observe_native_row(3, 0);
            let before = state;
            let mut transaction = DriveStripPointerTransaction::new();
            assert!(transaction.native_focus_result(true));
            let snapshot = state.provision_pointer_hover(decision.target, 0);
            assert_eq!(snapshot, before);
            assert!(transaction.model_focus_result(true));
            if fail_at_stage {
                assert!(!transaction.row_staging_result(false));
            } else {
                assert!(transaction.row_staging_result(true));
                assert!(!transaction.rebuild_scheduling_result(false));
            }
            assert!(transaction.rollback_required());
            state.restore_interaction(snapshot);
            assert_eq!(state.committed_pointer_position(), None);

            let mut retry = pointer_facts();
            retry.last_pointer_position = state.committed_pointer_position();
            assert!(route_drive_strip_pointer_move(retry).is_some());
        }
    }

    #[test]
    fn pointer_commit_gate_requires_every_native_hit_eligibility_step() {
        let mut transaction = DriveStripPointerTransaction::new();
        assert!(!transaction.can_commit_pointer());
        assert!(transaction.native_focus_result(true));
        assert!(transaction.model_focus_result(true));
        assert!(transaction.row_staging_result(true));
        assert!(!transaction.can_commit_pointer());
        assert!(transaction.rebuild_scheduling_result(true));
        assert!(transaction.can_commit_pointer());
    }

    #[test]
    fn production_transaction_executor_rolls_back_every_post_focus_failure() {
        use std::cell::Cell;

        fn run(
            provision: Result<bool, DriveStripPointerProvisionFailure>,
            staged: bool,
            scheduled: bool,
            committed: bool,
            model_restored: bool,
            row_records_restaged: bool,
            native_row_restored: bool,
        ) -> (DriveStripPointerTransactionOutcome, usize) {
            let rollbacks = Cell::new(0);
            let outcome = execute_drive_strip_pointer_transaction(
                || true,
                || provision,
                || staged,
                || scheduled,
                || committed,
                || {
                    rollbacks.set(rollbacks.get() + 1);
                    DriveStripPointerRollbackResult {
                        model_restored,
                        row_records_restaged,
                        native_row_restored,
                    }
                },
            );
            (outcome, rollbacks.get())
        }

        assert_eq!(
            run(Ok(true), true, true, true, true, true, true),
            (DriveStripPointerTransactionOutcome::Committed, 0)
        );
        let presentation_sinks = Cell::new(0);
        assert_eq!(
            execute_drive_strip_pointer_transaction(
                || true,
                || Ok(false),
                || {
                    presentation_sinks.set(presentation_sinks.get() + 1);
                    true
                },
                || {
                    presentation_sinks.set(presentation_sinks.get() + 1);
                    true
                },
                || true,
                || unreachable!("unchanged presentation must not roll back"),
            ),
            DriveStripPointerTransactionOutcome::Committed
        );
        assert_eq!(
            presentation_sinks.get(),
            0,
            "same-target motion must not prepare rows or schedule a close/resubmit"
        );
        for (provision, staged, scheduled, committed, failure) in [
            (
                Err(DriveStripPointerProvisionFailure::MissingModel),
                true,
                true,
                true,
                DriveStripPointerTransactionFailure::MissingModel,
            ),
            (
                Err(DriveStripPointerProvisionFailure::InvalidProvisionalFocus),
                true,
                true,
                true,
                DriveStripPointerTransactionFailure::InvalidProvisionalFocus,
            ),
            (
                Ok(true),
                false,
                true,
                true,
                DriveStripPointerTransactionFailure::RowRecordsStaged,
            ),
            (
                Ok(true),
                true,
                false,
                true,
                DriveStripPointerTransactionFailure::RebuildScheduled,
            ),
            (
                Ok(true),
                true,
                true,
                false,
                DriveStripPointerTransactionFailure::MissingModelBeforeCommit,
            ),
        ] {
            assert_eq!(
                run(provision, staged, scheduled, committed, true, false, false),
                (
                    DriveStripPointerTransactionOutcome::RolledBack {
                        failure,
                        rollback: DriveStripPointerRollbackResult {
                            model_restored: true,
                            row_records_restaged: false,
                            native_row_restored: false,
                        },
                    },
                    1,
                )
            );
        }
        assert_eq!(
            run(
                Err(DriveStripPointerProvisionFailure::MissingModel),
                true,
                true,
                true,
                false,
                false,
                true,
            ),
            (
                DriveStripPointerTransactionOutcome::RolledBack {
                    failure: DriveStripPointerTransactionFailure::MissingModel,
                    rollback: DriveStripPointerRollbackResult {
                        model_restored: false,
                        row_records_restaged: false,
                        native_row_restored: true,
                    },
                },
                1,
            )
        );
        assert_eq!(
            run(Ok(true), false, true, true, true, true, true),
            (
                DriveStripPointerTransactionOutcome::RolledBack {
                    failure: DriveStripPointerTransactionFailure::RowRecordsStaged,
                    rollback: DriveStripPointerRollbackResult {
                        model_restored: true,
                        row_records_restaged: true,
                        native_row_restored: true,
                    },
                },
                1,
            ),
            "successful row-record restaging must survive the rollback outcome diagnostics"
        );
        assert_eq!(
            execute_drive_strip_pointer_transaction(
                || false,
                || unreachable!("no model publication after native focus rejection"),
                || unreachable!(),
                || unreachable!(),
                || unreachable!(),
                || unreachable!("nothing was published to roll back"),
            ),
            DriveStripPointerTransactionOutcome::NativeFocusRejected
        );
    }

    #[test]
    fn foreign_background_and_invalid_client_windows_are_rejected() {
        for window in [
            DriveStripWindowFacts {
                hwnd_present: false,
                ..VALID_WINDOW
            },
            DriveStripWindowFacts {
                foreground_matches: false,
                ..VALID_WINDOW
            },
            DriveStripWindowFacts {
                same_process: false,
                ..VALID_WINDOW
            },
            DriveStripWindowFacts {
                client_geometry_valid: false,
                ..VALID_WINDOW
            },
            DriveStripWindowFacts {
                pointer_in_client: false,
                ..VALID_WINDOW
            },
        ] {
            let mut facts = pointer_facts();
            facts.window = window;
            assert_eq!(route_drive_strip_pointer_move(facts), None);
            assert_eq!(
                route_drive_strip_native_click(window, 0, 0, true, 3, -389.0, -210.0, BOUNDS,),
                None
            );
        }
    }

    #[test]
    fn physical_row_classifier_separates_ordinary_rows_from_drive_strip_validation() {
        assert_eq!(classify_picker_physical_row(0, Some(0)), None);
        assert_eq!(
            classify_picker_physical_row(1, Some(0)),
            Some(DriveStripActivationProvenance::OrdinaryRowPhysicalActivation)
        );
        assert_eq!(
            classify_picker_physical_row(0, None),
            Some(DriveStripActivationProvenance::OrdinaryRowPhysicalActivation)
        );
    }

    fn four_k_viewport_with_nonzero_screen_origin() -> DriveStripMovieViewport {
        DriveStripMovieViewport {
            client_origin_screen_x: 320.0,
            client_origin_screen_y: -120.0,
            client_width: 3840.0,
            client_height: 2160.0,
            movie_width: 1920.0,
            movie_height: 1080.0,
        }
    }

    #[test]
    fn event_client_and_live_screen_points_share_one_nonzero_origin_transform() {
        let viewport = four_k_viewport_with_nonzero_screen_origin();
        let raw_event_client = (1808.0, 639.0);
        let live_screen = (
            raw_event_client.0 + viewport.client_origin_screen_x,
            raw_event_client.1 + viewport.client_origin_screen_y,
        );
        let event_stage = viewport
            .client_point_to_movie_stage(raw_event_client.0, raw_event_client.1)
            .expect("event is inside the client viewport");
        let live_stage = viewport
            .screen_point_to_movie_stage(live_screen.0, live_screen.1)
            .expect("live pointer is inside the same client viewport");
        assert_eq!(event_stage, live_stage);
        assert!((event_stage.0 - -56.0).abs() < 0.01);
        assert!((event_stage.1 - -220.5).abs() < 0.01);
        assert_eq!(
            BOUNDS.classify(event_stage.0, event_stage.1, 3),
            Some(DriveStripFocus::CurrentPath)
        );
        assert_eq!(
            BOUNDS.classify(live_stage.0, live_stage.1, 3),
            Some(DriveStripFocus::CurrentPath)
        );
        assert_eq!(
            BOUNDS.classify(raw_event_client.0, raw_event_client.1, 3),
            None,
            "regression oracle: raw client pixels are not movie-stage coordinates"
        );
    }

    #[test]
    fn different_event_and_live_controls_remain_rejected_after_conversion() {
        let viewport = four_k_viewport_with_nonzero_screen_origin();
        let event_stage = viewport
            .client_point_to_movie_stage(1808.0, 639.0)
            .expect("path event");
        let live_client_x =
            ((-389.0 + viewport.movie_width * 0.5) / viewport.movie_width) * viewport.client_width;
        let live_client_y = ((-220.5 + viewport.movie_height * 0.5) / viewport.movie_height)
            * viewport.client_height;
        let live_stage = viewport
            .screen_point_to_movie_stage(
                live_client_x + viewport.client_origin_screen_x,
                live_client_y + viewport.client_origin_screen_y,
            )
            .expect("cell live pointer");
        assert_eq!(
            agree_drive_strip_click_targets(
                BOUNDS.classify(event_stage.0, event_stage.1, 3),
                BOUNDS.classify(live_stage.0, live_stage.1, 3),
            ),
            None
        );
    }

    #[test]
    fn invalid_viewport_origin_and_out_of_client_points_fail_closed() {
        let viewport = four_k_viewport_with_nonzero_screen_origin();
        assert_eq!(
            viewport.client_point_to_movie_stage(viewport.client_width, 639.0),
            None
        );
        assert_eq!(
            viewport.screen_point_to_movie_stage(
                viewport.client_origin_screen_x - 1.0,
                viewport.client_origin_screen_y + 639.0,
            ),
            None
        );
        for invalid in [
            DriveStripMovieViewport {
                client_origin_screen_x: f32::NAN,
                ..viewport
            },
            DriveStripMovieViewport {
                client_width: 0.0,
                ..viewport
            },
            DriveStripMovieViewport {
                movie_height: 0.0,
                ..viewport
            },
        ] {
            assert_eq!(invalid.client_point_to_movie_stage(1808.0, 639.0), None);
            assert_eq!(invalid.screen_point_to_movie_stage(2128.0, 519.0), None);
        }
    }

    #[test]
    fn event_and_live_click_targets_must_agree() {
        assert_eq!(
            agree_drive_strip_click_targets(
                Some(DriveStripFocus::Cell(1)),
                Some(DriveStripFocus::Cell(1))
            ),
            Some(DriveStripFocus::Cell(1))
        );
        assert_eq!(
            agree_drive_strip_click_targets(
                Some(DriveStripFocus::Cell(1)),
                Some(DriveStripFocus::CurrentPath)
            ),
            None
        );
        assert_eq!(
            agree_drive_strip_click_targets(Some(DriveStripFocus::Cell(1)), None),
            None
        );
    }

    #[test]
    fn shared_classifier_rejects_points_outside_drive_row_y_band() {
        assert_eq!(BOUNDS.classify(-389.0, BOUNDS.row_top - 0.1, 3), None);
        assert_eq!(
            BOUNDS.classify(-389.0, BOUNDS.row_top + BOUNDS.row_height, 3),
            None
        );
        assert_eq!(
            BOUNDS.classify(-389.0, BOUNDS.row_top + 0.1, 3),
            Some(DriveStripFocus::Cell(1))
        );
    }

    #[test]
    fn keyboard_and_unknown_provenance_are_each_armed_forwarded_and_cleared_once() {
        use std::cell::Cell;

        let arms = Cell::new(0);
        let forwards = Cell::new(0);
        let clears = Cell::new(0);
        forward_drive_strip_native_activation_once(
            DriveStripActivationProvenance::KeyboardOrPadAccept,
            |_| arms.set(arms.get() + 1),
            || forwards.set(forwards.get() + 1),
            || clears.set(clears.get() + 1),
        );
        assert_eq!(arms.get(), 1);
        assert_eq!(forwards.get(), 1);
        assert_eq!(clears.get(), 1);

        forward_drive_strip_native_activation_once(
            DriveStripActivationProvenance::UnknownNativeActivation,
            |_| arms.set(arms.get() + 1),
            || forwards.set(forwards.get() + 1),
            || clears.set(clears.get() + 1),
        );
        assert_eq!(arms.get(), 2);
        assert_eq!(forwards.get(), 2);
        assert_eq!(clears.get(), 2);
    }

    #[test]
    fn physical_provenance_is_consumed_or_cleared_once_without_replay() {
        use std::cell::{Cell, RefCell};

        for provenance in [
            DriveStripActivationProvenance::AcceptedPhysicalClick(DriveStripFocus::Cell(1)),
            DriveStripActivationProvenance::RejectedPhysicalClick,
            DriveStripActivationProvenance::OrdinaryRowPhysicalActivation,
        ] {
            for native_consumes in [true, false] {
                let pending = RefCell::new(None);
                let consumed = RefCell::new(Vec::new());
                let forwards = Cell::new(0);
                let clears = Cell::new(0);
                forward_drive_strip_native_activation_once(
                    provenance,
                    |provenance| *pending.borrow_mut() = Some(provenance),
                    || {
                        forwards.set(forwards.get() + 1);
                        if native_consumes {
                            if let Some(provenance) = pending.borrow_mut().take() {
                                consumed.borrow_mut().push(provenance);
                            }
                        }
                    },
                    || {
                        clears.set(clears.get() + 1);
                        pending.borrow_mut().take();
                    },
                );
                assert_eq!(forwards.get(), 1);
                assert_eq!(clears.get(), 1);
                assert_eq!(pending.into_inner(), None);
                assert_eq!(consumed.borrow().len(), usize::from(native_consumes));
            }
        }
    }

    #[test]
    fn exact_half_open_cell_and_path_bounds_do_not_overlap() {
        assert_eq!(
            BOUNDS.classify(-422.0, -210.0, 3),
            Some(DriveStripFocus::Cell(0))
        );
        assert_eq!(
            BOUNDS.classify(-390.0, -210.0, 3),
            Some(DriveStripFocus::Cell(1))
        );
        assert_eq!(BOUNDS.classify(-326.0, -210.0, 3), None);
        assert_eq!(
            BOUNDS.classify(-182.0, -210.0, 3),
            Some(DriveStripFocus::CurrentPath)
        );
        assert_eq!(BOUNDS.classify(418.0, -210.0, 3), None);
    }
}
