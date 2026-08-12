// Owner-zero refresh recovery. Included textually into `save_picker_menu.rs`; see the
// `include!` there. Split out only to keep that file under the repo's hard size limit.

/// Owner-zero pumps tolerated for one refresh generation before the picker gives its ownership of
/// the System rows back. The 2026-08-11 hover regression spun ~290 times per second for 125 s
/// (36,501 pumps) with the quit menu inert the whole time; a couple of seconds is already far more
/// than any legitimate removal->resubmit handoff needs.
const SAVE_PICKER_OWNER_ZERO_SPIN_LIMIT: usize = 600;

static SAVE_PICKER_OWNER_ZERO_SPIN_GENERATION: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_OWNER_ZERO_SPINS: AtomicUsize = AtomicUsize::new(0);

/// Count consecutive owner-zero pumps for one refresh generation. A new generation restarts the
/// count, so a healthy close/reopen never accumulates toward the limit.
fn save_picker_owner_zero_spin_tick(generation: usize) -> usize {
    if SAVE_PICKER_OWNER_ZERO_SPIN_GENERATION.swap(generation, Ordering::SeqCst) != generation {
        SAVE_PICKER_OWNER_ZERO_SPINS.store(1, Ordering::SeqCst);
        return 1;
    }
    SAVE_PICKER_OWNER_ZERO_SPINS.fetch_add(1, Ordering::SeqCst) + 1
}

pub(crate) fn save_picker_reset_owner_zero_spin_state() {
    SAVE_PICKER_OWNER_ZERO_SPIN_GENERATION.store(0, Ordering::SeqCst);
    SAVE_PICKER_OWNER_ZERO_SPINS.store(0, Ordering::SeqCst);
}

/// Name the exact conjunct that rejected the native-removal ticket. `owns_refresh` folds seven
/// separate predicates into one bool, so a rejection is otherwise indistinguishable at runtime.
fn save_picker_log_owner_zero_ticket_rejection(request: PickerRefreshRequest) {
    let Some(authority) = save_picker_native_removal_authority() else {
        append_autoload_debug(format_args!(
            "save-picker: owner-zero ticket ABSENT generation={} old_owner=0x{:x}; no native-removal authority was ever published",
            request.generation, request.dialog
        ));
        return;
    };
    let pending = save_picker_pending_resubmit_transition();
    let system_identity = save_picker_system_dialog_identity();
    let expected_system = PickerSystemDialogIdentity {
        dialog: authority.pending.system_dialog,
        generation: authority.pending.system_dialog_generation,
    };
    append_autoload_debug(format_args!(
        "save-picker: owner-zero ticket REJECTED generation={} old_owner=0x{:x} \
         coordinator_current={} pending_matches={} system_matches={} mode_active={} live_owner=0x{:x} \
         matches_refresh={} pending_refresh_gen={} pending_close_gen={} pending_old_dialog=0x{:x} \
         system_now={:?} system_expected=({:#x},{})",
        request.generation,
        request.dialog,
        picker_owner_lifetime().native_removal_authority_is_current(authority),
        pending == Some(authority.pending),
        system_identity == Some(expected_system),
        SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst),
        save_picker_live_profile_dialog(),
        picker_native_removal_matches_refresh(authority, request),
        authority.pending.refresh_owner_generation,
        authority.pending.refresh_close_generation,
        authority.pending.old_dialog,
        system_identity.map(|identity| (identity.dialog, identity.generation)),
        expected_system.dialog,
        expected_system.generation,
    ));
}

/// Give the System rows back when a refresh can no longer discharge. Leaving `SAVE_PICKER_MODE_ACTIVE`
/// latched with no picker window suppresses every underlying quit-menu activation, so the user sees
/// a menu whose buttons are all dead. Dropping picker ownership restores a usable menu; the picker
/// simply has to be reopened.
unsafe fn save_picker_release_wedged_owner_zero_refresh(request: PickerRefreshRequest) {
    let retired = retire_picker_refresh_request(request, false);
    clear_picker_pending_resubmit_transition();
    save_picker_set_reopen_pending(0);
    let reset = unsafe { system_quit_reset_profile_select_state("owner-zero-refresh-wedged") };
    er_telemetry::counters::SAVE_PICKER_OWNER_ZERO_LOOP_GUARD_MAX
        .fetch_max(SAVE_PICKER_OWNER_ZERO_SPIN_LIMIT, Ordering::SeqCst);
    save_picker_reset_owner_zero_spin_state();
    append_autoload_debug(format_args!(
        "save-picker: owner-zero refresh WEDGED past {SAVE_PICKER_OWNER_ZERO_SPIN_LIMIT} pumps generation={} old_owner=0x{:x} retired={retired} reset={reset}; releasing picker ownership so the System rows accept input again",
        request.generation, request.dialog
    ));
}
