/// One focusable control inside the combined drive/path row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStripFocus {
    Cell(usize),
    CurrentPath,
}

/// Which device currently owns the row's transient presentation. Keyboard focus is retained while
/// the pointer hovers, but pointer hover never becomes the keyboard Accept target.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DriveStripInputOwner {
    Pointer,
    #[default]
    Keyboard,
}

/// Pure interaction state shared by the host tests and the runtime adapter. Pointer position is
/// committed only after native focus, model presentation, and any required fresh-owner refresh
/// request have succeeded. ProfileSummary staging occurs later, after old-owner clear. Keyboard/pad
/// transitions clear transient hover ownership but retain the last
/// physical position, so an unmoved pointer cannot immediately steal focus back. Genuine pointer
/// absence clears the committed position so a same-pixel re-entry retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriveStripInteractionState {
    keyboard_focus: DriveStripFocus,
    keyboard_focus_deliberate: bool,
    pointer_hover: Option<DriveStripFocus>,
    owner: DriveStripInputOwner,
    observed_native_row: Option<usize>,
    committed_pointer_position: Option<u64>,
    presentation_dirty: bool,
    presentation_requires_fresh_owner: bool,
}

impl Default for DriveStripInteractionState {
    fn default() -> Self {
        Self {
            keyboard_focus: DriveStripFocus::CurrentPath,
            keyboard_focus_deliberate: false,
            pointer_hover: None,
            owner: DriveStripInputOwner::Keyboard,
            observed_native_row: None,
            committed_pointer_position: None,
            presentation_dirty: false,
            presentation_requires_fresh_owner: false,
        }
    }
}

impl DriveStripInteractionState {
    pub const fn keyboard_focus(self) -> DriveStripFocus {
        self.keyboard_focus
    }

    pub const fn pointer_hover(self) -> Option<DriveStripFocus> {
        self.pointer_hover
    }

    pub const fn owner(self) -> DriveStripInputOwner {
        self.owner
    }

    pub const fn presented_focus(self) -> DriveStripFocus {
        match (self.owner, self.pointer_hover) {
            (DriveStripInputOwner::Pointer, Some(focus)) => focus,
            _ => self.keyboard_focus,
        }
    }

    pub const fn committed_pointer_position(self) -> Option<u64> {
        self.committed_pointer_position
    }

    pub const fn observed_native_row(self) -> Option<usize> {
        self.observed_native_row
    }

    pub const fn presentation_dirty(self) -> bool {
        self.presentation_dirty
    }

    /// Whether the pending presentation change is one only a fresh owner can carry.
    ///
    /// A fresh-owner refresh natively CLOSES the live `05_010_ProfileSelect` window and resubmits
    /// it, so it is only ever legitimate for a change the user deliberately committed (keyboard/pad
    /// drive-strip navigation). Transient pointer hover must never set this: hovering is not a
    /// commit, and tearing the window down under the cursor reads to the user as Escape -- observed
    /// 2026-08-11, where one hover onto `CurrentPath` closed the picker and it never came back.
    /// Hover still marks `presentation_dirty`, so the next legitimate re-stage carries it.
    pub const fn presentation_requires_fresh_owner(self) -> bool {
        self.presentation_requires_fresh_owner
    }

    pub fn mark_presentation_staged(&mut self) {
        self.presentation_dirty = false;
        self.presentation_requires_fresh_owner = false;
    }

    /// Observe the game's native cursor rather than inferring vertical movement from Left/Right.
    /// A transition away from or keyboard re-entry into row 0 establishes keyboard ownership. A
    /// pointer-induced row focus is recorded through `provision_pointer_hover`, so the next sample
    /// observes row 0 as already pointer-owned instead of misclassifying it as keyboard entry.
    pub fn observe_native_row(&mut self, native_row: usize, drive_row: usize) -> bool {
        if self.observed_native_row == Some(native_row) {
            return false;
        }
        let previous_presented = self.presented_focus();
        let previous_owner = self.owner;
        let had_hover = self.pointer_hover.is_some();
        self.observed_native_row = Some(native_row);
        self.pointer_hover = None;
        self.owner = DriveStripInputOwner::Keyboard;
        if native_row == drive_row && !self.keyboard_focus_deliberate {
            self.keyboard_focus = DriveStripFocus::CurrentPath;
        }
        // First observation of the freshly constructed row does not change presentation: native
        // construction already populated the default keyboard CurrentPath geometry. Re-entry after
        // pointer ownership does change it and must be carried by a fresh owner.
        let presentation_changed = had_hover
            || previous_owner != self.owner
            || previous_presented != self.presented_focus();
        self.presentation_dirty |= presentation_changed;
        self.presentation_requires_fresh_owner |= presentation_changed;
        true
    }

    /// Mark a rejected/out-of-band pointer sample as absence. This deliberately clears the
    /// committed position so returning to the same physical pixel is a new entry.
    pub fn pointer_left(&mut self) -> bool {
        let previous_presented = self.presented_focus();
        let previous_owner = self.owner;
        let had_hover = self.pointer_hover.take().is_some();
        let had_position = self.committed_pointer_position.take().is_some();
        let had_pointer = had_hover || had_position || self.owner == DriveStripInputOwner::Pointer;
        self.owner = DriveStripInputOwner::Keyboard;
        let changed = had_pointer
            && (previous_presented != self.presented_focus() || previous_owner != self.owner);
        // Pointer absence is not a committed change; it may not close the live window.
        self.presentation_dirty |= changed;
        changed
    }

    /// Provisional model/presentation step of a pointer transaction. The position is intentionally
    /// not consumed here. Callers restore the returned snapshot if a refresh request fails.
    pub fn provision_pointer_hover(&mut self, target: DriveStripFocus, drive_row: usize) -> Self {
        let snapshot = *self;
        let presentation_changed = self.owner != DriveStripInputOwner::Pointer
            || self.pointer_hover != Some(target)
            || self.presented_focus() != target;
        self.pointer_hover = Some(target);
        self.owner = DriveStripInputOwner::Pointer;
        self.observed_native_row = Some(drive_row);
        // Hover is transient presentation, never a commit: it may dirty the model but must not
        // authorize the destructive fresh-owner refresh that closes the live window.
        self.presentation_dirty |= presentation_changed;
        snapshot
    }

    pub fn commit_pointer_position(&mut self, position: u64) {
        self.committed_pointer_position = Some(position);
        self.presentation_dirty = false;
        self.presentation_requires_fresh_owner = false;
    }

    pub fn restore_interaction(&mut self, snapshot: Self) {
        *self = snapshot;
    }

    pub fn set_keyboard_focus(&mut self, focus: DriveStripFocus) -> bool {
        if self.keyboard_focus == focus
            && self.owner == DriveStripInputOwner::Keyboard
            && self.keyboard_focus_deliberate
        {
            return false;
        }
        self.keyboard_focus = focus;
        self.keyboard_focus_deliberate = true;
        self.owner = DriveStripInputOwner::Keyboard;
        self.pointer_hover = None;
        // A deliberate keyboard/pad drive-strip commit is the one transition a fresh owner exists
        // to carry: the user asked for a different drive, so re-staging the rows is the point.
        self.presentation_dirty = true;
        self.presentation_requires_fresh_owner = true;
        true
    }
}

impl SavePickerModel {
    /// Keyboard/pad Accept target. Pointer hover is intentionally excluded.
    pub fn drive_strip_focus(&self) -> Option<DriveStripFocus> {
        self.has_drive_row()
            .then_some(self.drive_strip_interaction.keyboard_focus())
            .filter(|focus| match focus {
                DriveStripFocus::Cell(cell) => *cell < self.drive_strip_cell_count(),
                DriveStripFocus::CurrentPath => true,
            })
            .or_else(|| self.has_drive_row().then_some(DriveStripFocus::CurrentPath))
    }

    /// Target used only to constrain the row's animated cursor during populate.
    pub fn drive_strip_presented_focus(&self) -> Option<DriveStripFocus> {
        if !self.has_drive_row() {
            return None;
        }
        let focus = self.drive_strip_interaction.presented_focus();
        match focus {
            DriveStripFocus::Cell(cell) if cell < self.drive_strip_cell_count() => Some(focus),
            DriveStripFocus::Cell(_) => Some(DriveStripFocus::CurrentPath),
            DriveStripFocus::CurrentPath => Some(focus),
        }
    }

    pub fn drive_strip_input_owner(&self) -> DriveStripInputOwner {
        self.drive_strip_interaction.owner()
    }

    pub fn drive_strip_pointer_hover(&self) -> Option<DriveStripFocus> {
        self.drive_strip_interaction.pointer_hover()
    }

    pub fn drive_strip_pointer_position(&self) -> Option<u64> {
        self.drive_strip_interaction.committed_pointer_position()
    }

    pub fn drive_strip_interaction_snapshot(&self) -> DriveStripInteractionState {
        self.drive_strip_interaction
    }

    pub fn drive_strip_presentation_dirty(&self) -> bool {
        self.drive_strip_interaction.presentation_dirty()
    }

    pub fn drive_strip_presentation_requires_fresh_owner(&self) -> bool {
        self.drive_strip_interaction
            .presentation_requires_fresh_owner()
    }

    pub fn mark_drive_strip_presentation_staged(&mut self) {
        self.drive_strip_interaction.mark_presentation_staged();
    }

    pub fn observe_drive_strip_native_row(&mut self, native_row: usize) -> bool {
        let Some(drive_row) = self.drive_row() else {
            return false;
        };
        self.drive_strip_interaction
            .observe_native_row(native_row, drive_row)
    }

    pub fn drive_strip_pointer_left(&mut self) -> bool {
        self.drive_strip_interaction.pointer_left()
    }

    pub fn provision_drive_strip_pointer_hover(
        &mut self,
        target: DriveStripFocus,
    ) -> Option<DriveStripInteractionState> {
        let drive_row = self.drive_row()?;
        let valid = match target {
            DriveStripFocus::Cell(cell) => cell < self.drive_strip_cell_count(),
            DriveStripFocus::CurrentPath => true,
        };
        valid.then(|| {
            self.drive_strip_interaction
                .provision_pointer_hover(target, drive_row)
        })
    }

    pub fn commit_drive_strip_pointer_position(&mut self, position: u64) {
        self.drive_strip_interaction
            .commit_pointer_position(position);
    }

    pub fn rollback_drive_strip_interaction(&mut self, snapshot: DriveStripInteractionState) {
        self.drive_strip_interaction.restore_interaction(snapshot);
    }

    /// Deliberate keyboard/pad Left/Right focus change without activation.
    pub fn set_drive_strip_focus(&mut self, focus: DriveStripFocus) -> bool {
        let valid = match focus {
            DriveStripFocus::Cell(cell) => cell < self.drive_strip_cell_count(),
            DriveStripFocus::CurrentPath => self.has_drive_row(),
        };
        valid && self.drive_strip_interaction.set_keyboard_focus(focus)
    }

    pub fn focus_active_drive_cell(&mut self) -> bool {
        self.drive_strip_active_cell()
            .is_some_and(|cell| self.set_drive_strip_focus(DriveStripFocus::Cell(cell)))
    }

    /// Resolve Accept from keyboard subfocus without consulting transient pointer hover.
    pub fn drive_strip_focused_activation(&self) -> Option<crate::DriveStripActivation> {
        self.drive_strip_focus().map(Into::into)
    }

    /// Traverse visible cells and then the path without wrapping or activating either.
    pub fn move_drive_strip_focus(&mut self, forward: bool) -> bool {
        let count = self.drive_strip_cell_count();
        if count == 0 {
            return false;
        }
        let current = self
            .drive_strip_focus()
            .unwrap_or(DriveStripFocus::CurrentPath);
        let next = match (current, forward) {
            (DriveStripFocus::Cell(cell), true) if cell + 1 < count => {
                DriveStripFocus::Cell(cell + 1)
            }
            (DriveStripFocus::Cell(_), true) => DriveStripFocus::CurrentPath,
            (DriveStripFocus::CurrentPath, false) => DriveStripFocus::Cell(count - 1),
            (DriveStripFocus::Cell(cell), false) if cell > 0 => DriveStripFocus::Cell(cell - 1),
            _ => current,
        };
        self.set_drive_strip_focus(next)
    }
}

#[cfg(test)]
mod drive_strip_focus_tests {
    use super::*;

    const BOUNDS: crate::DriveStripPointerBounds = crate::DriveStripPointerBounds {
        first_cell_left: -422.0,
        cell_pitch: 32.0,
        cell_width: 32.0,
        path_left: -182.0,
        path_width: 600.0,
        row_top: -236.0,
        row_height: 39.0,
    };
    const VALID_WINDOW: crate::DriveStripWindowFacts = crate::DriveStripWindowFacts {
        hwnd_present: true,
        foreground_matches: true,
        same_process: true,
        client_geometry_valid: true,
        pointer_in_client: true,
    };

    fn pointer(position: u64, stage_x: f32) -> crate::DriveStripPointerSample {
        crate::DriveStripPointerSample {
            window: VALID_WINDOW,
            packed_position: position,
            stage_x,
            stage_y: BOUNDS.row_top + 0.1,
        }
    }

    fn publish_pointer_plan(
        model: &mut SavePickerModel,
        plan: crate::DriveStripPumpPlan,
    ) -> DriveStripFocus {
        let decision = plan.pointer_decision.expect("pointer route decision");
        model
            .provision_drive_strip_pointer_hover(decision.target)
            .expect("valid pointer target");
        model.commit_drive_strip_pointer_position(decision.commit_pointer_position);
        decision.target
    }

    fn model(current: &str) -> SavePickerModel {
        SavePickerModel {
            current_dir: PathBuf::from(current),
            extension: "sl2".to_owned(),
            extensions: vec!["sl2".to_owned()],
            entries: Vec::new(),
            scroll_offset: 0,
            cursor: 0,
            drive_strip_offset: 0,
            drive_strip_interaction: DriveStripInteractionState::default(),
            status_message: None,
            rejected_path_text: None,
            edge_scroll_ticks: 0,
            edge_scroll_repeats: 0,
            edge_scroll_direction: 0,
            drives: ["A:\\", "B:\\", "C:\\", "D:\\"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            last_dir_per_drive: HashMap::new(),
            intent: PickerIntent::LoadSource,
        }
    }

    #[test]
    fn initial_keyboard_row_focus_accepts_current_path() {
        let mut model = model("A:\\saves");
        model.observe_drive_strip_native_row(0);
        assert_eq!(
            model.drive_strip_focus(),
            Some(DriveStripFocus::CurrentPath)
        );
        assert_eq!(
            model.drive_strip_focused_activation(),
            Some(crate::DriveStripActivation::OpenCurrentPath)
        );
    }

    #[test]
    fn inactive_drive_pointer_hover_does_not_activate_or_replace_keyboard_focus() {
        let mut model = model("A:\\saves");
        let before = model.current_dir().to_path_buf();
        let _snapshot = model
            .provision_drive_strip_pointer_hover(DriveStripFocus::Cell(2))
            .expect("valid hover");
        model.commit_drive_strip_pointer_position(7);
        assert_eq!(model.current_dir(), before.as_path());
        assert_eq!(
            model.drive_strip_focus(),
            Some(DriveStripFocus::CurrentPath)
        );
        assert_eq!(
            model.drive_strip_pointer_hover(),
            Some(DriveStripFocus::Cell(2))
        );
        assert_eq!(
            model.drive_strip_input_owner(),
            DriveStripInputOwner::Pointer
        );
        assert!(model.activate_drive_strip_cell(2));
        assert_eq!(model.current_drive_root(), PathBuf::from("C:\\"));
        assert_eq!(
            model.drive_strip_focus(),
            Some(DriveStripFocus::CurrentPath),
            "pointer activation changes the active drive, not keyboard/pad subfocus"
        );
    }

    #[test]
    fn same_target_pointer_motion_changes_only_committed_position() {
        let mut model = model("A:\\saves");
        model.observe_drive_strip_native_row(0);
        model
            .provision_drive_strip_pointer_hover(DriveStripFocus::Cell(2))
            .expect("first target");
        assert!(model.drive_strip_presentation_dirty());
        model.commit_drive_strip_pointer_position(7);
        assert!(!model.drive_strip_presentation_dirty());

        model
            .provision_drive_strip_pointer_hover(DriveStripFocus::Cell(2))
            .expect("same target remains valid");
        assert!(
            !model.drive_strip_presentation_dirty(),
            "same-target motion must not request presentation replacement"
        );
        model.commit_drive_strip_pointer_position(8);
        assert_eq!(model.drive_strip_pointer_position(), Some(8));
        assert_eq!(
            model.drive_strip_pointer_hover(),
            Some(DriveStripFocus::Cell(2))
        );
    }

    #[test]
    fn stale_pointer_hover_down_up_accept_uses_keyboard_current_path() {
        let mut model = model("A:\\saves");
        model.observe_drive_strip_native_row(0);
        let _snapshot = model
            .provision_drive_strip_pointer_hover(DriveStripFocus::Cell(2))
            .expect("valid hover");
        model.commit_drive_strip_pointer_position(7);
        assert!(model.observe_drive_strip_native_row(1));
        assert!(model.observe_drive_strip_native_row(0));
        assert!(
            model.drive_strip_presentation_dirty(),
            "keyboard re-entry must replace the row that was populated for pointer ownership"
        );
        assert_eq!(model.drive_strip_pointer_hover(), None);
        assert_eq!(
            model.drive_strip_input_owner(),
            DriveStripInputOwner::Keyboard
        );
        assert_eq!(
            model.drive_strip_focused_activation(),
            Some(crate::DriveStripActivation::OpenCurrentPath)
        );
    }

    /// A fresh-owner refresh natively CLOSES the live `05_010_ProfileSelect` window. Regression for
    /// 2026-08-11: hovering the `CurrentPath` control queued one (`reason=drive-strip-presentation`),
    /// the window closed like Escape, the reopen never landed, and picker mode stayed latched so
    /// every quit-menu row activation was suppressed for the remaining 36,501 pumps. Pointer hover
    /// may dirty the presentation; it may never demand the owner be replaced.
    #[test]
    fn pointer_hover_never_requests_a_fresh_owner_but_keyboard_commit_does() {
        let mut model = model("A:\\saves");
        model.observe_drive_strip_native_row(0);
        model.mark_drive_strip_presentation_staged();
        assert!(!model.drive_strip_presentation_requires_fresh_owner());

        // Hover across every drive cell and onto CurrentPath: the exact motion that closed the menu.
        for (index, stage_x) in [
            BOUNDS.first_cell_left + 0.1,
            BOUNDS.first_cell_left + BOUNDS.cell_pitch + 0.1,
            BOUNDS.first_cell_left + BOUNDS.cell_pitch * 2.0 + 0.1,
            BOUNDS.path_left + 0.1,
        ]
        .into_iter()
        .enumerate()
        {
            let plan = crate::orchestrate_drive_strip_pump(
                &mut model,
                0,
                true,
                None,
                Some(pointer(0x1000 + index as u64, stage_x)),
                BOUNDS,
            )
            .expect("drive row");
            assert!(
                !plan.presentation_requires_fresh_owner,
                "hover at stage_x={stage_x} demanded a window-closing refresh"
            );
            let decision = plan.pointer_decision.expect("pointer route decision");
            model
                .provision_drive_strip_pointer_hover(decision.target)
                .expect("valid pointer target");
            // Pin the pre-fix behaviour this test exists to forbid: hover genuinely DOES dirty the
            // presentation, so the old `presentation_dirty`-only gate scheduled a window-closing
            // fresh-owner refresh right here. The dirt is fine; closing the window over it was not.
            assert!(
                model.drive_strip_presentation_dirty(),
                "hover at stage_x={stage_x} should still mark presentation dirty for a later stage"
            );
            assert!(
                !model.drive_strip_presentation_requires_fresh_owner(),
                "dirty hover presentation must not escalate to a fresh-owner demand"
            );
            model.commit_drive_strip_pointer_position(decision.commit_pointer_position);
            assert!(
                !model.drive_strip_presentation_requires_fresh_owner(),
                "hover at stage_x={stage_x} left a fresh-owner demand latched"
            );
            let _ = index;
        }
        assert_eq!(
            model.drive_strip_pointer_hover(),
            Some(DriveStripFocus::CurrentPath)
        );

        // A deliberate keyboard commit is the transition a fresh owner exists to carry.
        let plan = crate::orchestrate_drive_strip_pump(
            &mut model,
            0,
            true,
            Some(true),
            Some(pointer(0x2000, BOUNDS.path_left + 0.1)),
            BOUNDS,
        )
        .expect("drive row");
        assert!(plan.keyboard_navigation);
        assert!(plan.presentation_needs_stage);
        assert!(
            plan.presentation_requires_fresh_owner,
            "keyboard drive-strip navigation must still re-stage through a fresh owner"
        );
        model.mark_drive_strip_presentation_staged();
        assert!(!model.drive_strip_presentation_requires_fresh_owner());
    }

    #[test]
    fn production_pump_seam_keeps_stationary_pointer_from_stealing_keyboard_row() {
        let mut model = model("A:\\saves");
        let position = 0x1234;
        let cell_x = BOUNDS.first_cell_left + BOUNDS.cell_pitch * 2.0 + 0.1;
        let first = crate::orchestrate_drive_strip_pump(
            &mut model,
            0,
            true,
            None,
            Some(pointer(position, cell_x)),
            BOUNDS,
        )
        .expect("drive row");
        assert_eq!(
            publish_pointer_plan(&mut model, first),
            DriveStripFocus::Cell(2)
        );

        let down = crate::orchestrate_drive_strip_pump(
            &mut model,
            1,
            true,
            None,
            Some(pointer(position, cell_x)),
            BOUNDS,
        )
        .expect("drive row");
        assert!(down.native_row_changed);
        assert_eq!(down.pointer_decision, None);
        assert!(!down.pointer_absent);
        assert_eq!(model.drive_strip_pointer_position(), Some(position));
        assert_eq!(model.drive_strip_pointer_hover(), None);
        assert_eq!(
            model.drive_strip_input_owner(),
            DriveStripInputOwner::Keyboard
        );

        let up = crate::orchestrate_drive_strip_pump(
            &mut model,
            0,
            true,
            None,
            Some(pointer(position, cell_x)),
            BOUNDS,
        )
        .expect("drive row");
        assert!(up.native_row_changed);
        assert_eq!(up.pointer_decision, None);
        assert_eq!(
            crate::orchestrate_drive_strip_activation(
                &mut model,
                crate::DriveStripActivationProvenance::KeyboardOrPadAccept,
            ),
            crate::DriveStripActivationEffect::RequestPathEditor
        );

        let moved = crate::orchestrate_drive_strip_pump(
            &mut model,
            0,
            true,
            None,
            Some(pointer(position + 1, cell_x)),
            BOUNDS,
        )
        .expect("drive row");
        assert_eq!(
            moved.pointer_decision.map(|decision| decision.target),
            Some(DriveStripFocus::Cell(2)),
            "real physical movement may reclaim pointer hover"
        );
        publish_pointer_plan(&mut model, moved);

        let left = crate::orchestrate_drive_strip_pump(&mut model, 0, true, None, None, BOUNDS)
            .expect("drive row");
        assert!(left.pointer_absent);
        assert_eq!(model.drive_strip_pointer_position(), None);
        let reentered = crate::orchestrate_drive_strip_pump(
            &mut model,
            0,
            true,
            None,
            Some(pointer(position + 1, cell_x)),
            BOUNDS,
        )
        .expect("drive row");
        assert!(
            reentered.pointer_decision.is_some(),
            "same-pixel re-entry routes"
        );
        assert_eq!(
            publish_pointer_plan(&mut model, reentered),
            DriveStripFocus::Cell(2)
        );
        assert_eq!(
            model.drive_strip_input_owner(),
            DriveStripInputOwner::Pointer
        );
        assert_eq!(
            model.drive_strip_pointer_hover(),
            Some(DriveStripFocus::Cell(2))
        );
    }

    #[test]
    fn pure_source_route_semantics_cover_all_row_kinds() {
        use std::cell::{Cell, RefCell};

        fn forward_and_activate(
            model: &RefCell<SavePickerModel>,
            model_row: usize,
            provenance: crate::DriveStripActivationProvenance,
        ) -> (usize, usize, Vec<crate::PickerNativeActivationEffect>, bool) {
            let terminals = Cell::new(0);
            let suppressions = Cell::new(0);
            let effects = RefCell::new(Vec::new());
            let decision = crate::route_picker_source_activation(
                true,
                true,
                Some(&mut model.borrow_mut()),
                Some(model_row),
                provenance,
            );
            terminals.set(terminals.get() + 1);
            suppressions.set(suppressions.get() + 1);
            if let crate::PickerSourceDecision::Effect(effect) = decision {
                effects.borrow_mut().push(effect);
            }
            (
                terminals.get(),
                suppressions.get(),
                effects.into_inner(),
                false,
            )
        }

        let drive_model = RefCell::new(model("A:\\saves"));
        let cell_x = BOUNDS.first_cell_left + BOUNDS.cell_pitch * 2.0 + 0.1;
        let accepted_target = crate::agree_drive_strip_click_targets(
            crate::route_drive_strip_native_click(
                VALID_WINDOW,
                0,
                0,
                true,
                4,
                cell_x,
                BOUNDS.row_top + 0.1,
                BOUNDS,
            ),
            BOUNDS.classify(cell_x, BOUNDS.row_top + 0.1, 4),
        );
        assert_eq!(
            forward_and_activate(
                &drive_model,
                0,
                crate::DriveStripActivationProvenance::physical_click(accepted_target),
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::DriveSelected(2)],
                false,
            )
        );
        assert_eq!(
            drive_model.borrow().current_drive_root(),
            PathBuf::from("C:\\")
        );

        let mut path_model = model("A:\\saves");
        assert!(path_model.set_drive_strip_focus(DriveStripFocus::Cell(1)));
        let path_model = RefCell::new(path_model);
        let path_x = BOUNDS.path_left + 0.1;
        let accepted_path = crate::agree_drive_strip_click_targets(
            crate::route_drive_strip_native_click(
                VALID_WINDOW,
                0,
                0,
                true,
                4,
                path_x,
                BOUNDS.row_top + 0.1,
                BOUNDS,
            ),
            BOUNDS.classify(path_x, BOUNDS.row_top + 0.1, 4),
        );
        assert_eq!(
            forward_and_activate(
                &path_model,
                0,
                crate::DriveStripActivationProvenance::physical_click(accepted_path),
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::RequestPathEditor],
                false,
            )
        );
        assert_eq!(
            path_model.borrow().drive_strip_focus(),
            Some(DriveStripFocus::Cell(1))
        );

        let keyboard_model = RefCell::new(model("A:\\saves"));
        assert_eq!(
            forward_and_activate(
                &keyboard_model,
                0,
                crate::DriveStripActivationProvenance::KeyboardOrPadAccept,
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::RequestPathEditor],
                false,
            )
        );

        let invalid_window = crate::DriveStripWindowFacts {
            hwnd_present: false,
            ..VALID_WINDOW
        };
        let rejected_targets = [
            crate::agree_drive_strip_click_targets(
                crate::route_drive_strip_native_click(
                    invalid_window,
                    0,
                    0,
                    true,
                    4,
                    cell_x,
                    BOUNDS.row_top + 0.1,
                    BOUNDS,
                ),
                BOUNDS.classify(cell_x, BOUNDS.row_top + 0.1, 4),
            ),
            crate::agree_drive_strip_click_targets(
                crate::route_drive_strip_native_click(
                    VALID_WINDOW,
                    0,
                    0,
                    true,
                    4,
                    cell_x,
                    BOUNDS.row_top - 0.1,
                    BOUNDS,
                ),
                BOUNDS.classify(cell_x, BOUNDS.row_top + 0.1, 4),
            ),
            crate::agree_drive_strip_click_targets(
                Some(DriveStripFocus::Cell(2)),
                Some(DriveStripFocus::CurrentPath),
            ),
        ];
        for rejected_target in rejected_targets {
            let mut rejected_model = model("A:\\saves");
            assert!(rejected_model.set_drive_strip_focus(DriveStripFocus::Cell(1)));
            let rejected_model = RefCell::new(rejected_model);
            let before = rejected_model.borrow().current_dir().to_path_buf();
            assert_eq!(
                forward_and_activate(
                    &rejected_model,
                    0,
                    crate::DriveStripActivationProvenance::physical_click(rejected_target),
                ),
                (1, 1, Vec::new(), false)
            );
            assert_eq!(rejected_model.borrow().current_dir(), before.as_path());
            assert_eq!(
                rejected_model.borrow().drive_strip_focus(),
                Some(DriveStripFocus::Cell(1)),
                "rejected physical provenance never falls back to keyboard subfocus"
            );
        }

        let up_model = RefCell::new(model("A:\\saves"));
        let up_row = up_model.borrow().parent_row().expect("up row");
        assert_eq!(
            forward_and_activate(
                &up_model,
                up_row,
                crate::classify_picker_physical_row(up_row, Some(0)).expect("ordinary up row"),
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::Model(
                    PickerActivation::Repopulate,
                )],
                false,
            )
        );

        let folder_path = PathBuf::from("A:\\saves\\folder");
        let mut folder_model = model("A:\\saves");
        folder_model.entries.push(PickerEntry::Dir {
            name: "folder".to_owned(),
            path: folder_path.clone(),
        });
        let folder_row = folder_model.entry_row_base();
        let folder_model = RefCell::new(folder_model);
        assert_eq!(
            forward_and_activate(
                &folder_model,
                folder_row,
                crate::classify_picker_physical_row(folder_row, Some(0))
                    .expect("ordinary folder row"),
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::Model(
                    PickerActivation::Repopulate,
                )],
                false,
            )
        );
        assert_eq!(folder_model.borrow().current_dir(), folder_path.as_path());

        let file_path = PathBuf::from("A:\\saves\\ER0000.sl2");
        let mut file_model = model("A:\\saves");
        file_model.entries.push(PickerEntry::File {
            name: "ER0000.sl2".to_owned(),
            path: file_path.clone(),
            modified: None,
            chars: Vec::new(),
        });
        let file_row = file_model.entry_row_base();
        let file_model = RefCell::new(file_model);
        assert_eq!(
            forward_and_activate(
                &file_model,
                file_row,
                crate::classify_picker_physical_row(file_row, Some(0)).expect("ordinary file row"),
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::Model(
                    PickerActivation::PickedFile(file_path.clone()),
                )],
                false,
            )
        );
        assert_eq!(
            forward_and_activate(
                &file_model,
                file_row,
                crate::DriveStripActivationProvenance::KeyboardOrPadAccept,
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::Model(
                    PickerActivation::PickedFile(file_path),
                )],
                false,
            ),
            "keyboard/pad Accept retains ordinary-row model activation"
        );

        let mut new_model = model("A:\\saves");
        new_model.intent = PickerIntent::SaveDestination {
            loaded_file_name: "ER0000.sl2".to_owned(),
            loaded_path: PathBuf::from("A:\\active\\ER0000.sl2"),
        };
        let new_row = new_model.new_file_row().expect("new row");
        let new_target = PathBuf::from("A:\\saves").join("ER0000.sl2");
        let new_model = RefCell::new(new_model);
        assert_eq!(
            forward_and_activate(
                &new_model,
                new_row,
                crate::classify_picker_physical_row(new_row, Some(0)).expect("ordinary new row"),
            ),
            (
                1,
                1,
                vec![crate::PickerNativeActivationEffect::Model(
                    PickerActivation::PickedNewFile(new_target),
                )],
                false,
            )
        );
    }

    #[test]
    fn delayed_callback_after_source_scope_is_a_named_late_rejection() {
        let mut model = model("A:\\saves");
        let decision = crate::route_picker_source_activation(
            true,
            true,
            Some(&mut model),
            Some(0),
            crate::DriveStripActivationProvenance::KeyboardOrPadAccept,
        );
        assert_eq!(
            decision,
            crate::PickerSourceDecision::Effect(
                crate::PickerNativeActivationEffect::RequestPathEditor,
            )
        );
        assert_eq!(
            crate::reject_picker_late_callback(),
            crate::PickerSourceDecision::Rejected(crate::PickerSourceRejection::LateCallback),
        );
    }

    #[test]
    fn pure_route_smoke_handles_thirty_two_source_decisions() {
        let mut model = model("A:\\saves");
        let mut effects = 0usize;
        let mut unknown = 0usize;
        let mut native_forwards = 0usize;
        for event in 0..32 {
            let provenance = if event % 2 == 0 {
                crate::DriveStripActivationProvenance::KeyboardOrPadAccept
            } else {
                crate::DriveStripActivationProvenance::AcceptedPhysicalClick(
                    crate::DriveStripFocus::Cell(event % 4),
                )
            };
            match crate::route_picker_source_activation(
                true,
                true,
                Some(&mut model),
                Some(0),
                provenance,
            ) {
                crate::PickerSourceDecision::Effect(_) => effects += 1,
                crate::PickerSourceDecision::ForwardNative => native_forwards += 1,
                crate::PickerSourceDecision::Rejected(
                    crate::PickerSourceRejection::UnknownSource,
                ) => unknown += 1,
                other => panic!("event {event} was not terminal effect: {other:?}"),
            }
        }
        assert_eq!(effects, 32);
        assert_eq!(unknown, 0);
        assert_eq!(native_forwards, 0);
    }

    #[test]
    fn non_picker_forwards_once_and_picker_unknown_suppresses() {
        let mut model = model("A:\\saves");
        assert_eq!(
            crate::route_picker_source_activation(
                false,
                true,
                Some(&mut model),
                Some(0),
                crate::DriveStripActivationProvenance::KeyboardOrPadAccept,
            ),
            crate::PickerSourceDecision::ForwardNative,
        );
        assert_eq!(
            crate::route_picker_source_activation(
                true,
                false,
                Some(&mut model),
                Some(0),
                crate::DriveStripActivationProvenance::KeyboardOrPadAccept,
            ),
            crate::PickerSourceDecision::ForwardNative,
        );
        assert_eq!(
            crate::route_picker_source_activation(
                true,
                true,
                Some(&mut model),
                Some(0),
                crate::DriveStripActivationProvenance::UnknownNativeActivation,
            ),
            crate::PickerSourceDecision::Rejected(crate::PickerSourceRejection::UnknownSource),
        );
    }

    #[test]
    fn deliberate_keyboard_subfocus_survives_vertical_exit_and_reentry() {
        let mut model = model("A:\\saves");
        model.observe_drive_strip_native_row(0);
        assert!(model.move_drive_strip_focus(false));
        let focus = model.drive_strip_focus();
        model.observe_drive_strip_native_row(1);
        model.observe_drive_strip_native_row(0);
        assert_eq!(model.drive_strip_focus(), focus);
        assert_eq!(
            model.drive_strip_input_owner(),
            DriveStripInputOwner::Keyboard
        );
    }

    #[test]
    fn keyboard_traverses_visible_drives_then_current_path_and_accepts_exact_target() {
        let mut model = model("D:\\saves");
        assert!(model.set_drive_strip_focus(DriveStripFocus::Cell(3)));
        let before = model.current_dir().to_path_buf();
        assert!(model.move_drive_strip_focus(true));
        assert_eq!(
            model.drive_strip_focus(),
            Some(DriveStripFocus::CurrentPath)
        );
        assert_eq!(
            model.drive_strip_focused_activation(),
            Some(crate::DriveStripActivation::OpenCurrentPath)
        );
        assert_eq!(model.current_dir(), before.as_path());
        assert!(model.move_drive_strip_focus(false));
        assert_eq!(model.drive_strip_focus(), Some(DriveStripFocus::Cell(3)));
        assert_eq!(
            model.drive_strip_focused_activation(),
            Some(crate::DriveStripActivation::SelectCell(3))
        );
        assert_eq!(model.current_dir(), before.as_path());
    }
}
