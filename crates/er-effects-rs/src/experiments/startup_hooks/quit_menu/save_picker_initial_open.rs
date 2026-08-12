pub(crate) fn execute_picker_initial_open_sequence_with<A, D, M>(
    arm: impl FnOnce() -> Option<A>,
    start_dir: impl FnOnce() -> Option<D>,
    build_model: impl FnOnce(D) -> Option<M>,
    stage_rows: impl FnOnce(&M) -> bool,
    publish_state: impl FnOnce(&mut A, M) -> bool,
    native_open: impl FnOnce() -> bool,
    commit: impl FnOnce(A),
) -> bool {
    let Some(mut attempt) = arm() else {
        return false;
    };
    let Some(start_dir) = start_dir() else {
        return false;
    };
    let Some(model) = build_model(start_dir) else {
        return false;
    };
    if !stage_rows(&model) {
        return false;
    }
    if !publish_state(&mut attempt, model) {
        return false;
    }
    if !native_open() {
        return false;
    }
    commit(attempt);
    true
}

pub(crate) struct PickerInitialOpenAttemptGuard {
    save_swap: Option<SystemQuitSaveSwapArmGuard>,
    action_obj: usize,
    published_system: Option<PickerSystemDialogIdentity>,
    state_published: bool,
    committed: bool,
}

impl PickerInitialOpenAttemptGuard {
    pub(crate) fn new(save_swap: SystemQuitSaveSwapArmGuard, action_obj: usize) -> Self {
        Self {
            save_swap: Some(save_swap),
            action_obj,
            published_system: None,
            state_published: false,
            committed: false,
        }
    }

    pub(crate) fn publish_model(
        &mut self,
        model: crate::experiments::save_picker::SavePickerModel,
    ) -> bool {
        let system_dialog = unsafe {
            safe_read_usize(self.action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET)
        }
        .unwrap_or(0);
        let Some(system_identity) = save_picker_try_publish_initial_system_dialog(system_dialog)
        else {
            append_autoload_debug(format_args!(
                "save-picker: initial state publication rejected action=0x{:x} system=0x{system_dialog:x}; exact System identity was not exclusively publishable",
                self.action_obj
            ));
            return false;
        };
        self.published_system = Some(system_identity);
        self.state_published = true;
        *crate::experiments::save_picker::active_save_picker_lock() = Some(model);
        SAVE_PICKER_MODE_ACTIVE.store(1, Ordering::SeqCst);
        SAVE_PICKER_ACTION_OBJ.store(self.action_obj, Ordering::SeqCst);
        save_picker_set_reopen_pending(0);
        clear_picker_refresh_request();
        clear_path_editor_return_reopen_request();
        clear_picker_pending_resubmit_transition();
        save_picker_set_open_slots_pending(0);
        true
    }

    pub(crate) fn commit(mut self) -> SystemQuitSaveSwapArmIdentity {
        self.committed = true;
        self.save_swap
            .take()
            .expect("initial picker attempt owns one save-swap arm")
            .commit()
    }

    fn rollback_published_state(&mut self) {
        if !self.state_published {
            return;
        }
        let Some(system_identity) = self.published_system else {
            return;
        };
        if SAVE_PICKER_ACTION_OBJ.load(Ordering::SeqCst) != self.action_obj
            || save_picker_system_dialog_identity() != Some(system_identity)
        {
            append_autoload_debug(format_args!(
                "save-picker: stale initial-open rollback refused action=0x{:x} system=0x{:x}/{}; newer picker identity is authoritative",
                self.action_obj, system_identity.dialog, system_identity.generation
            ));
            return;
        }
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
        let _ = SAVE_PICKER_ACTION_OBJ.compare_exchange(
            self.action_obj,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        let _ = save_picker_clear_exact_system_dialog(system_identity);
        save_picker_set_reopen_pending(0);
        clear_picker_refresh_request();
        clear_path_editor_return_reopen_request();
        clear_picker_pending_resubmit_transition();
        save_picker_set_open_slots_pending(0);
        clear_picker_presentation();
    }
}

impl Drop for PickerInitialOpenAttemptGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Abort the exact save arm first, following the established save-swap -> presentation ->
        // model/System cleanup lock order. Its generation check prevents an old guard from touching
        // a newer arm.
        if let Some(save_swap) = self.save_swap.take() {
            drop(save_swap);
        }
        self.rollback_published_state();
    }
}
