// Drive/path strip menu-pump. Included textually into `save_picker_menu.rs`; see the
// `include!` there. Split out only to keep that file under the repo's hard size limit.

#[cfg(test)]
#[path = "drive_strip_hit_tests.rs"]
mod drive_strip_hit_tests;

fn drive_strip_nav_pressed_mask(nav_edges: usize) -> usize {
    let mut pressed = 0;
    if nav_edges & crate::experiments::SAVE_PICKER_NAV_LEFT_MASK != 0 {
        pressed |= SAVE_PICKER_DRIVE_STRIP_LEFT_MASK;
    }
    if nav_edges & crate::experiments::SAVE_PICKER_NAV_RIGHT_MASK != 0 {
        pressed |= SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK;
    }
    pressed
}

fn save_picker_schedule_drive_strip_refresh(dialog: usize) -> bool {
    save_picker_schedule_refresh_request(dialog, "drive-strip-presentation")
}

unsafe fn save_picker_rollback_drive_strip_pointer(
    token: PickerProfileRunToken,
    previous_native_row: usize,
    snapshot: er_save_picker::DriveStripInteractionState,
    native_focus_changed: bool,
) -> er_save_picker::DriveStripPointerRollbackResult {
    let model_restored = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        if let Some(model) = guard.as_mut() {
            model.rollback_drive_strip_interaction(snapshot);
            true
        } else {
            false
        }
    };
    // No ProfileSummary transport occurs while the old owner is live. Rollback restores only model
    // and native-row ownership; any already-coalesced refresh will stage this restored state later,
    // after owner-zero.
    let row_records_restaged = false;
    let native_rollback = !native_focus_changed
        || unsafe { save_picker_set_native_row_focus(token, previous_native_row) };
    append_autoload_debug(format_args!(
        "save-picker: drive/path pointer transaction rolled back previous_native_row={previous_native_row} model_restored={model_restored} row_records_restaged=false native_rollback={native_rollback}; pointer position remains retryable"
    ));
    er_save_picker::DriveStripPointerRollbackResult {
        model_restored,
        row_records_restaged,
        native_row_restored: native_rollback,
    }
}

/// Menu-pump-owned drive/path focus. Native cursor transitions establish keyboard ownership;
/// pointer movement owns only transient hover. Physical LButton activation is deliberately absent:
/// the native event/activation transaction is its sole owner.
pub(crate) unsafe fn save_picker_menu_pump_drive_strip_mouse(token: PickerProfileRunToken) {
    let dialog = token.dialog;
    if !save_picker_profile_token_still_current(token)
        || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0
        || save_picker_path_editor_blocks_profile_refresh()
        || save_picker_resubmit_pending()
    {
        let _ = crate::experiments::save_picker_take_user_nav_edges();
        return;
    }
    crate::experiments::ensure_save_picker_user_nav_input_hooks_installed();
    let pressed =
        drive_strip_nav_pressed_mask(crate::experiments::save_picker_take_user_nav_edges());
    let Some(cursor) = (unsafe { save_picker_native_cursor(token) }) else {
        return;
    };
    let Some(model_row) = save_picker_model_row_from_native_cursor(cursor) else {
        return;
    };
    let (drive_row, controls_visible) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return;
        };
        let Some(drive_row) = model.drive_row() else {
            return;
        };
        (drive_row, model.status_message().is_none())
    };
    let bounds = save_picker_drive_strip_pointer_bounds(drive_row);
    let pointer = unsafe { save_picker_validated_game_pointer() }.map(|pointer| {
        er_save_picker::DriveStripPointerSample {
            window: pointer.window,
            packed_position: pointer.packed_position,
            stage_x: pointer.stage_x,
            stage_y: pointer.stage_y,
        }
    });
    let keyboard_move_forward = (model_row == drive_row
        && pressed & (SAVE_PICKER_DRIVE_STRIP_LEFT_MASK | SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK) != 0)
        .then_some(pressed & SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK != 0);
    let plan = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_mut() else {
            return;
        };
        er_save_picker::orchestrate_drive_strip_pump(
            model,
            model_row,
            controls_visible,
            keyboard_move_forward,
            pointer,
            bounds,
        )
    };
    let Some(plan) = plan else {
        return;
    };

    // The pure seam has already established keyboard ownership and absence state. Publish that
    // presentation before a later pointer transaction. A failed keyboard navigation rolls back to
    // the post-native-transition snapshot; pointer absence remains dirty and retryable.
    if plan.presentation_needs_stage && model_row == drive_row {
        // Only a deliberate keyboard/pad commit may close the live window. Hover-only dirt stages
        // silently and rides the next committed refresh, because a fresh-owner refresh is a native
        // window CLOSE and the user reads that as Escape.
        if plan.presentation_requires_fresh_owner
            && !save_picker_schedule_drive_strip_refresh(dialog)
        {
            if let Some(snapshot) = plan.keyboard_snapshot {
                let mut guard = crate::experiments::save_picker::active_save_picker_lock();
                if let Some(model) = guard.as_mut() {
                    model.rollback_drive_strip_interaction(snapshot);
                }
            }
            return;
        }
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        if let Some(model) = guard.as_mut() {
            model.mark_drive_strip_presentation_staged();
        }
    }
    if plan.keyboard_navigation || plan.pointer_absent {
        return;
    }
    let Some(decision) = plan.pointer_decision else {
        return;
    };

    let snapshot = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return;
        };
        model.drive_strip_interaction_snapshot()
    };
    let native_focus_changed = decision.native_row_focus.is_some();
    let outcome = er_save_picker::execute_drive_strip_pointer_transaction(
        || !native_focus_changed || unsafe { save_picker_set_native_row_focus(token, drive_row) },
        || {
            let mut guard = crate::experiments::save_picker::active_save_picker_lock();
            let Some(model) = guard.as_mut() else {
                return Err(er_save_picker::DriveStripPointerProvisionFailure::MissingModel);
            };
            model
                .provision_drive_strip_pointer_hover(decision.target)
                .map(|_| model.drive_strip_presentation_dirty())
                .ok_or(er_save_picker::DriveStripPointerProvisionFailure::InvalidProvisionalFocus)
        },
        || {
            crate::experiments::save_picker::active_save_picker_lock()
                .as_ref()
                .is_some()
        },
        || {
            // Pointer hover is not a commit. Scheduling a fresh-owner refresh here natively closes
            // the live 05_010 window under the cursor, which the user experiences as the menu
            // closing like Escape (observed 2026-08-11: one hover onto CurrentPath closed the
            // picker, the reopen never landed, and picker mode stayed latched so every quit-menu
            // button was suppressed). Hover keeps native row focus only; the dirty presentation
            // rides the next deliberate commit.
            let requires_fresh_owner = crate::experiments::save_picker::active_save_picker_lock()
                .as_ref()
                .is_some_and(|model| model.drive_strip_presentation_requires_fresh_owner());
            if requires_fresh_owner {
                save_picker_schedule_drive_strip_refresh(dialog)
            } else {
                true
            }
        },
        || {
            let mut guard = crate::experiments::save_picker::active_save_picker_lock();
            if let Some(model) = guard.as_mut() {
                model.commit_drive_strip_pointer_position(decision.commit_pointer_position);
                true
            } else {
                false
            }
        },
        || unsafe {
            save_picker_rollback_drive_strip_pointer(
                token,
                model_row,
                snapshot,
                native_focus_changed,
            )
        },
    );
    match outcome {
        er_save_picker::DriveStripPointerTransactionOutcome::Committed => {
            append_autoload_debug(format_args!(
                "save-picker: drive/path pointer committed target={:?} native_cursor={cursor} drive_row={drive_row} native_focus_changed={native_focus_changed}",
                decision.target
            ));
        }
        er_save_picker::DriveStripPointerTransactionOutcome::NativeFocusRejected => {
            append_autoload_debug(format_args!(
                "save-picker: native drive-row focus rejected cursor={cursor} drive_row={drive_row}; pointer position not committed"
            ));
        }
        er_save_picker::DriveStripPointerTransactionOutcome::RolledBack { failure, rollback } => {
            append_autoload_debug(format_args!(
                "save-picker: drive/path pointer transaction failed failure={failure:?} model_restored={} row_records_restaged={} native_row_restored={} target={:?}",
                rollback.model_restored,
                rollback.row_records_restaged,
                rollback.native_row_restored,
                decision.target
            ));
        }
    }
}

fn save_picker_scrollbar_packed_state(current: usize, page: usize, total: usize) -> usize {
    (current.min(0xffff) & 0xffff)
        | ((page.min(0xffff) & 0xffff) << 16)
        | ((total.min(0xffff) & 0xffff) << 32)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
enum ScrollbarDispatchRejectReason {
    MissingVtable = 1,
    VtableOutsideGameImage = 2,
    MissingTarget = 3,
    TargetOutsideGameImage = 4,
    PurecallTarget = 5,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScrollbarDispatchRejection {
    reason: ScrollbarDispatchRejectReason,
    vtable: usize,
    target: usize,
}

/// Validate the exact dispatch that `FUN_14074dcc0 -> FUN_140735a60 -> FUN_140733340` performs.
/// The last function receives `scrollbar+8`, loads its table, then calls `[table+8]`. After an
/// in-place ProfileLoad rebuild this proxy may still be readable while the dispatch slot is null;
/// calling either native scrollbar setter in that state executes address zero.
fn save_picker_scrollbar_dispatch_preflight_with(
    base: usize,
    scrollbar: usize,
    mut read: impl FnMut(usize) -> Option<usize>,
) -> Result<usize, ScrollbarDispatchRejection> {
    let vtable = read(scrollbar + SCROLLBAR_VISIBLE_PROXY_OFFSET).unwrap_or(0);
    if vtable == 0 {
        return Err(ScrollbarDispatchRejection {
            reason: ScrollbarDispatchRejectReason::MissingVtable,
            vtable,
            target: 0,
        });
    }
    // Read the exact slot even when the table itself is stale/non-image. The fault-safe production
    // reader turns an unmapped table into `None`; a still-mapped torn-down table (the crash case)
    // records its null slot rather than hiding it behind only the table-range rejection.
    let target = read(vtable + SCROLLBAR_VISIBLE_PROXY_DISPATCH_SLOT).unwrap_or(0);
    if target == 0 {
        return Err(ScrollbarDispatchRejection {
            reason: ScrollbarDispatchRejectReason::MissingTarget,
            vtable,
            target,
        });
    }
    if !vtable_in_game_image(vtable, base) {
        return Err(ScrollbarDispatchRejection {
            reason: ScrollbarDispatchRejectReason::VtableOutsideGameImage,
            vtable,
            target,
        });
    }
    if !vtable_in_game_image(target, base) {
        return Err(ScrollbarDispatchRejection {
            reason: ScrollbarDispatchRejectReason::TargetOutsideGameImage,
            vtable,
            target,
        });
    }
    if crate::constants::dispatch_target_is_purecall(target, base) {
        return Err(ScrollbarDispatchRejection {
            reason: ScrollbarDispatchRejectReason::PurecallTarget,
            vtable,
            target,
        });
    }
    Ok(target)
}

/// Production adapter for the guarded native setter pair. Tests inject both the fault-safe reader
/// and setter sinks into this same function, so a rejected dispatch proves neither setter ran.
fn save_picker_apply_native_scrollbar_with(
    base: usize,
    scrollbar: usize,
    total: i32,
    current: i32,
    read: impl FnMut(usize) -> Option<usize>,
    mut set_total: impl FnMut(usize, i32) -> bool,
    mut set_position: impl FnMut(usize, i32) -> bool,
) -> Result<Option<usize>, ScrollbarDispatchRejection> {
    let target = save_picker_scrollbar_dispatch_preflight_with(base, scrollbar, read)?;
    if !set_total(scrollbar, total) || !set_position(scrollbar, current) {
        return Ok(None);
    }
    Ok(Some(target))
}
