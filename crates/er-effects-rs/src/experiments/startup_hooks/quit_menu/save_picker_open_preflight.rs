pub(crate) const PICKER_OPEN_SOURCE_ACTION_THUNK_SUPPRESSED: usize = 1;
pub(crate) const PICKER_OPEN_SOURCE_CONTROLLER_SUPPRESSED: usize = 2;
const PICKER_OPEN_SOURCE_COALESCED: usize = 3;
const PICKER_OPEN_SOURCE_REJECTED: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerLoadSourceOpenFacts {
    pub(crate) mode_active: bool,
    pub(crate) profile_owner: usize,
    pub(crate) profile_vtable: usize,
    pub(crate) expected_profile_vtable: usize,
    pub(crate) live_owner_authorized: bool,
    pub(crate) activation_system: usize,
    pub(crate) activation_action: usize,
    pub(crate) tracked_system: usize,
    pub(crate) tracked_action: usize,
    pub(crate) owner_zero_resubmit_pending: bool,
    pub(crate) exact_parent_authority: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerLoadSourceOpenDecision {
    Initial,
    CoalescedLive,
    CoalescedOwnerZero,
    Rejected,
}

pub(crate) fn classify_picker_load_source_open(
    facts: PickerLoadSourceOpenFacts,
) -> PickerLoadSourceOpenDecision {
    let exact_picker_identity = facts.mode_active
        && facts.activation_system != 0
        && facts.activation_system == facts.tracked_system
        && facts.activation_action != 0
        && facts.activation_action == facts.tracked_action;
    if exact_picker_identity
        && facts.profile_owner != 0
        && facts.expected_profile_vtable != 0
        && facts.profile_vtable == facts.expected_profile_vtable
        && facts.live_owner_authorized
    {
        return PickerLoadSourceOpenDecision::CoalescedLive;
    }
    if exact_picker_identity && facts.profile_owner == 0 && facts.owner_zero_resubmit_pending {
        return PickerLoadSourceOpenDecision::CoalescedOwnerZero;
    }
    if !facts.mode_active
        && facts.profile_owner == 0
        && !facts.owner_zero_resubmit_pending
        && facts.tracked_system == 0
        && facts.tracked_action == 0
        && facts.exact_parent_authority
    {
        return PickerLoadSourceOpenDecision::Initial;
    }
    PickerLoadSourceOpenDecision::Rejected
}

/// Pumps a transition may hold the System rows without its signature changing before it is treated
/// as abandoned. Measured against real transitions in the 2026-08-11 logs: a healthy close ->
/// owner-zero -> resubmit completed in ~240 ms, and the maintenance pump runs on every MenuWindow
/// post, so a legitimate transition moves its signature within a handful of pumps. This bound is
/// two orders of magnitude looser than that, and only a transition that has genuinely stopped can
/// reach it.
const SAVE_PICKER_TRANSITION_STALL_LIMIT: usize = 600;

static SAVE_PICKER_TRANSITION_LAST_SIGNATURE: std::sync::Mutex<Option<[usize; 6]>> =
    std::sync::Mutex::new(None);
static SAVE_PICKER_TRANSITION_STALL_PUMPS: AtomicUsize = AtomicUsize::new(0);

/// Whether the owning transition has made no observable progress for too many pumps.
///
/// Called once per maintenance pump. Any signature change resets the count, so a transition that is
/// still moving -- however slowly -- is never declared stalled.
fn picker_transition_is_stalled() -> bool {
    let signature = save_picker_transition_signature();
    let mut last = SAVE_PICKER_TRANSITION_LAST_SIGNATURE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.as_ref() != Some(&signature) {
        *last = Some(signature);
        SAVE_PICKER_TRANSITION_STALL_PUMPS.store(0, Ordering::SeqCst);
        return false;
    }
    let pumps = SAVE_PICKER_TRANSITION_STALL_PUMPS.fetch_add(1, Ordering::SeqCst) + 1;
    if pumps < SAVE_PICKER_TRANSITION_STALL_LIMIT {
        return false;
    }
    er_telemetry::counters::SAVE_PICKER_TRANSITION_STALLS.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker: transition STALLED for {pumps} pumps with an unchanged signature {signature:x?}; treating it as abandoned so the picker state can be released"
    ));
    SAVE_PICKER_TRANSITION_STALL_PUMPS.store(0, Ordering::SeqCst);
    *last = None;
    true
}

/// Consecutive pumps the SAME orphaned facts must persist before the state is released.
///
/// Acting on a single observation races legitimate handoffs. Observed 2026-08-12: the path-editor
/// no-close return armed its reopen at 06:25:10.94 and the parent owner hit zero at 06:25:11.33 --
/// and in that window `transition_owned` had already been consumed while `tracked_system` was still
/// set, so the release fired mid-handoff (`stalled=false mode_active=false owner=0x0
/// tracked_system=0x190034080`) and reset the reopen out from under the user, soft-locking the menu.
/// A real wedge holds its facts forever; a handoff resolves in a few pumps.
const SAVE_PICKER_ORPHAN_PERSIST_PUMPS: usize = 600;

static SAVE_PICKER_ORPHAN_LAST_FACTS: std::sync::Mutex<Option<PickerOrphanFacts>> =
    std::sync::Mutex::new(None);
static SAVE_PICKER_ORPHAN_PUMPS: AtomicUsize = AtomicUsize::new(0);

/// Whether these exact orphaned facts have persisted long enough to be a wedge rather than a
/// transient. Any change in the facts restarts the count, so a progressing handoff never trips it.
fn picker_orphan_state_persisted(facts: PickerOrphanFacts) -> bool {
    let mut last = SAVE_PICKER_ORPHAN_LAST_FACTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.as_ref() != Some(&facts) {
        *last = Some(facts);
        SAVE_PICKER_ORPHAN_PUMPS.store(1, Ordering::SeqCst);
        return false;
    }
    let pumps = SAVE_PICKER_ORPHAN_PUMPS.fetch_add(1, Ordering::SeqCst) + 1;
    if pumps < SAVE_PICKER_ORPHAN_PERSIST_PUMPS {
        return false;
    }
    SAVE_PICKER_ORPHAN_PUMPS.store(0, Ordering::SeqCst);
    *last = None;
    true
}

/// Facts the orphaned-state invariant judges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOrphanFacts {
    pub(crate) mode_active: bool,
    pub(crate) profile_owner: usize,
    pub(crate) owner_dangling: bool,
    pub(crate) tracked_system: usize,
    pub(crate) tracked_action: usize,
    pub(crate) real_windows_hidden: bool,
    pub(crate) transition_owned: bool,
}

/// Whether the picker has left state behind that can only reject work.
///
/// Picker liveness is carried by THREE independent pieces of state, and each teardown route has
/// managed to leak a different one:
///   * `SAVE_PICKER_MODE_ACTIVE` -- gates `picker_system_row_activation_is_inert`, so a latched mode
///     eats every System row before the open preflight is even reached (native Escape out of the
///     picker, 2026-08-11: `owner=0x179f46080 vtable=0x0 mode=true`).
///   * `SYSTEM_QUIT_PROFILE_SELECT_WINDOW` -- `Initial` requires it to be zero, so a leaked owner
///     rejects every open (path-editor accept path, 2026-08-11: `owner=0x18beca080`).
///   * the tracked identity `save_picker_system_dialog_identity()` / `SAVE_PICKER_ACTION_OBJ` --
///     `Initial` requires BOTH to be zero. Escape out of the software keyboard leaks exactly these
///     two while correctly clearing the other two (2026-08-11: `mode_active=false owner=0x0
///     owner_zero_pending=false exact_parent_authority=true tracked_system=0x18e6a0080
///     tracked_action=0x18dfda2f0`), which is why an owner-only invariant could not see it.
///
/// Judging all three together is the point: fixing them one route at a time is what let this bug
/// come back three times. With no transition in flight, none of these leftovers can do anything
/// except refuse the user, so the sanctioned reset is always the right answer.
///
/// Fail CLOSED on `transition_owned` -- an in-flight resubmit, refresh, path-editor return, initial
/// open, or reset legitimately owns this state mid-flight.
pub(crate) fn picker_state_is_orphaned(facts: PickerOrphanFacts) -> bool {
    if facts.transition_owned {
        return false;
    }
    if facts.mode_active {
        // Mode is only meaningful with a live window behind it.
        return facts.owner_dangling;
    }
    // Mode is already off: any surviving owner or tracked identity is pure rejection fuel.
    //
    // `real_windows_hidden` only counts when there is NO owner at all. Hiding IngameTop and
    // OptionSetting is done FOR the picker, so with a live window present it is the CORRECT state,
    // and treating it as a wedge caused a 2,240-iteration ping-pong on 2026-08-11: release ->
    // restore -> the next `05_010` post legitimately re-hid -> release again, with the facts each
    // time reading `mode_active=false owner=0x1900ba080 owner_dangling=false tracked=0x0/0x0` -- a
    // perfectly healthy picker whose mode simply had not been republished yet. Only `owner == 0`
    // proves nothing is left to hide them for.
    facts.owner_dangling
        || facts.tracked_system != 0
        || facts.tracked_action != 0
        || (facts.real_windows_hidden && facts.profile_owner == 0)
}

/// Facts the stale-owner invariant judges. Kept separate from `PickerLoadSourceOpenFacts` so the
/// decision is a pure function of exactly what it reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerStaleOwnerFacts {
    pub(crate) profile_owner: usize,
    pub(crate) profile_vtable: usize,
    pub(crate) expected_profile_vtable: usize,
    pub(crate) live_owner_authorized: bool,
    pub(crate) owner_zero_resubmit_pending: bool,
}

/// Whether the published ProfileSelect owner is a DANGLING pointer that must be republished to zero.
///
/// The owner global is only meaningful while the object it names is still a `ProfileLoadDialog`; a
/// window's first qword is its vtable, so a mismatch means the allocation was freed and the heap
/// reused it. That state is not merely untidy -- `classify_picker_load_source_open` requires
/// `profile_owner == 0` before it will grant an `Initial` open, so a leaked owner rejects every
/// future load-source open permanently, and the preflight keeps dereferencing freed memory.
///
/// Observed 2026-08-11: a path-editor (SoftwareKeyboard) cancel resubmitted the 05_010 page, the
/// unwind to `02_040_OptionSetting` destroyed it through a route whose finalize boundaries were all
/// `capture=None disposition=Foreign`, so no `CompareRemove` ever published zero. Owner stayed at
/// `0x18beca080` whose vtable slot read `0x18beaa080` (a heap pointer, not `0x142b229f8`), and 17
/// consecutive `Load Character from File` clicks were refused -- the menu appeared to be dead.
///
/// A NULL vtable read counts as dangling, and is in fact the strongest evidence available: a live
/// `ProfileLoadDialog` can never have a null first qword, so zero means the page was unmapped or
/// zeroed (`safe_read_usize` also reports an unreadable address as `0`). Treating zero as merely
/// "unreadable, leave it alone" was the 2026-08-11 miss -- the Escape teardown left
/// `owner=0x179f46080 owner_vtable=0x0 mode=true`, and the release refused to fire on the exact
/// state it existed for. Only the heap-reuse variant (`vtable=0x18beaa080`) was being caught.
///
/// Fail CLOSED on the states where a real transition still owns the pointer: a live/leased token or
/// an in-flight owner-zero transition. Those, not the vtable read, are the safety conditions.
pub(crate) fn picker_owner_is_dangling(facts: PickerStaleOwnerFacts) -> bool {
    facts.profile_owner != 0
        && facts.expected_profile_vtable != 0
        && facts.profile_vtable != facts.expected_profile_vtable
        && !facts.live_owner_authorized
        && !facts.owner_zero_resubmit_pending
}

fn picker_load_source_parent_authority(action_obj: usize, activation_system: usize) -> bool {
    let controller = system_quit_controller_of_action_alias(action_obj);
    let expected_controller = system_quit_row_controller(QuitRow::LoadSaveProfiles);
    action_obj >= PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET
        && activation_system >= 0x10000
        && activation_system == SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst)
        && controller != 0
        && controller == expected_controller
        && controller.checked_add(PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET)
            == Some(action_obj)
}

unsafe fn picker_load_source_open_facts(action_obj: usize) -> PickerLoadSourceOpenFacts {
    let activation_system =
        unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(0);
    let profile_owner = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let profile_vtable = if profile_owner >= 0x10000 {
        unsafe { safe_read_usize(profile_owner) }.unwrap_or(0)
    } else {
        0
    };
    let expected_profile_vtable = game_module_base()
        .ok()
        .and_then(|base| base.checked_add(PROFILE_LOAD_DIALOG_VTABLE_RVA))
        .unwrap_or(0);
    PickerLoadSourceOpenFacts {
        mode_active: SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
        profile_owner,
        profile_vtable,
        expected_profile_vtable,
        live_owner_authorized: picker_owner_lifetime()
            .current_live_token(profile_owner, profile_vtable, expected_profile_vtable)
            .is_some(),
        activation_system,
        activation_action: action_obj,
        tracked_system: save_picker_system_dialog_identity().map_or(0, |identity| identity.dialog),
        tracked_action: SAVE_PICKER_ACTION_OBJ.load(Ordering::SeqCst),
        owner_zero_resubmit_pending: save_picker_resubmit_pending()
            || SAVE_PICKER_REFRESH_PENDING_DIALOG.load(Ordering::SeqCst) != 0
            || er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG
                .load(Ordering::SeqCst)
                != 0,
        exact_parent_authority: picker_load_source_parent_authority(action_obj, activation_system),
    }
}

fn record_picker_open_preflight_facts(source: usize, facts: PickerLoadSourceOpenFacts) {
    er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_LAST_SOURCE.store(source, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_LAST_OWNER
        .store(facts.profile_owner, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_LAST_SYSTEM
        .store(facts.activation_system, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_LAST_ACTION
        .store(facts.activation_action, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_LAST_VTABLE
        .store(facts.profile_vtable, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_LAST_TRACKED_SYSTEM
        .store(facts.tracked_system, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_LAST_TRACKED_ACTION
        .store(facts.tracked_action, Ordering::SeqCst);
}

pub(crate) fn record_picker_system_row_activation_suppression(
    action_obj: usize,
    row: Option<QuitRow>,
    source: usize,
) {
    let facts = unsafe { picker_load_source_open_facts(action_obj) };
    er_telemetry::counters::SAVE_PICKER_SYSTEM_ROW_ACTIVATION_SUPPRESSIONS
        .fetch_add(1, Ordering::SeqCst);
    if row == Some(QuitRow::LoadSaveProfiles) {
        er_telemetry::counters::SAVE_PICKER_DUPLICATE_UNDERLYING_ACTIVATION_SUPPRESSIONS
            .fetch_add(1, Ordering::SeqCst);
    }
    record_picker_open_preflight_facts(source, facts);
    append_autoload_debug(format_args!(
        "save-picker: underlying System-row activation suppressed source={source} row={row:?} owner=0x{:x} owner_vtable=0x{:x} system=0x{:x} action=0x{:x} mode={} owner_zero_pending={}",
        facts.profile_owner,
        facts.profile_vtable,
        facts.activation_system,
        facts.activation_action,
        facts.mode_active,
        facts.owner_zero_resubmit_pending,
    ));
}

pub(crate) enum PickerLoadSourceOpenPreflightWith<G> {
    Initial(G),
    Coalesced(PickerLoadSourceOpenDecision, PickerLoadSourceOpenFacts),
    Rejected(PickerLoadSourceOpenFacts),
}

/// Production-used sequencing seam: classify before any mutation, then claim the exclusive initial
/// open boundary or reclassify after a lost claim. Only `Initial` permits restore/stage/native work.
pub(crate) fn picker_load_source_open_preflight_with<G>(
    facts: PickerLoadSourceOpenFacts,
    begin_initial: impl FnOnce() -> Option<G>,
    refresh_facts: impl FnOnce() -> PickerLoadSourceOpenFacts,
) -> PickerLoadSourceOpenPreflightWith<G> {
    match classify_picker_load_source_open(facts) {
        decision @ (PickerLoadSourceOpenDecision::CoalescedLive
        | PickerLoadSourceOpenDecision::CoalescedOwnerZero) => {
            PickerLoadSourceOpenPreflightWith::Coalesced(decision, facts)
        }
        PickerLoadSourceOpenDecision::Rejected => {
            PickerLoadSourceOpenPreflightWith::Rejected(facts)
        }
        PickerLoadSourceOpenDecision::Initial => match begin_initial() {
            Some(guard) => PickerLoadSourceOpenPreflightWith::Initial(guard),
            None => {
                let refreshed = refresh_facts();
                let decision = classify_picker_load_source_open(refreshed);
                if matches!(
                    decision,
                    PickerLoadSourceOpenDecision::CoalescedLive
                        | PickerLoadSourceOpenDecision::CoalescedOwnerZero
                ) {
                    PickerLoadSourceOpenPreflightWith::Coalesced(decision, refreshed)
                } else {
                    PickerLoadSourceOpenPreflightWith::Rejected(refreshed)
                }
            }
        },
    }
}

pub(crate) type PickerLoadSourceOpenPreflight =
    PickerLoadSourceOpenPreflightWith<PickerResetTransactionGuard<'static>>;

/// Read the current owner facts and release the pointer if it is provably dangling. Returns whether
/// a release happened, so the caller re-reads its facts afterwards.
///
/// The republish goes through the sanctioned compare-set (`expected == the dangling dialog`), so a
/// concurrent legitimate publication wins instead of being clobbered.
pub(crate) unsafe fn release_dangling_profile_owner() -> bool {
    let profile_owner = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let profile_vtable = if profile_owner >= 0x10000 {
        unsafe { safe_read_usize(profile_owner) }.unwrap_or(0)
    } else {
        0
    };
    let expected_profile_vtable = game_module_base()
        .ok()
        .and_then(|base| base.checked_add(PROFILE_LOAD_DIALOG_VTABLE_RVA))
        .unwrap_or(0);
    let facts = PickerStaleOwnerFacts {
        profile_owner,
        profile_vtable,
        expected_profile_vtable,
        live_owner_authorized: picker_owner_lifetime()
            .current_live_token(profile_owner, profile_vtable, expected_profile_vtable)
            .is_some(),
        owner_zero_resubmit_pending: save_picker_resubmit_pending()
            || SAVE_PICKER_REFRESH_PENDING_DIALOG.load(Ordering::SeqCst) != 0
            || er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG
                .load(Ordering::SeqCst)
                != 0,
    };
    if !picker_owner_is_dangling(facts) {
        return false;
    }
    let disposition = save_picker_path_editor_publish_owner_if_current(profile_owner, 0);
    let released = matches!(disposition, PickerOwnerPublicationDisposition::Published(_));
    if released {
        er_telemetry::counters::SAVE_PICKER_STALE_OWNER_RELEASES.fetch_add(1, Ordering::SeqCst);
        er_telemetry::counters::SAVE_PICKER_STALE_OWNER_LAST_DIALOG
            .store(profile_owner, Ordering::SeqCst);
        er_telemetry::counters::SAVE_PICKER_STALE_OWNER_LAST_VTABLE
            .store(profile_vtable, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "save-picker: released DANGLING ProfileSelect owner=0x{profile_owner:x} vtable=0x{profile_vtable:x} expected=0x{expected_profile_vtable:x} disposition={disposition:?} released={released}; the window was freed without a removal boundary and was rejecting every load-source open"
    ));
    released
}

/// Release the whole picker state when its window is dead but `SAVE_PICKER_MODE_ACTIVE` is still
/// latched. Pointer-free apart from the owner vtable probe, so any MenuWindow post may run it.
///
/// Releasing only the owner pointer is NOT enough, and assuming otherwise was the 2026-08-11 miss.
/// `picker_system_row_activation_is_inert` is `quit_row_controller && (picker_mode_active ||
/// transition_owned)`, and it gates the row BEFORE the open preflight is ever reached -- so a
/// latched mode eats every System row (Load Character from File, Return to Desktop, all of them)
/// and the preflight never gets a chance to repair anything. Minimal repro: open Load Character
/// from File, press Escape, reopen -- Escape tears the window down natively without any of our
/// removal boundaries firing, leaving `owner=0x179f46080 owner_vtable=0x0 mode=true` and a quit
/// menu where nothing responds.
///
/// Fails closed: any owned transition (resubmit, refresh, path-editor return, initial open, reset)
/// keeps every piece of the state, because those are the moments it is legitimately mid-flight.
pub(crate) unsafe fn release_orphaned_picker_state() -> bool {
    // A transition owning the rows is legitimate ONLY while it is progressing. Deferring to it
    // unconditionally is what wedged the 2026-08-11 third loop: `owner_zero_pending=true` was stuck
    // on, which both held the rows inert directly AND silenced every release below, because they all
    // deferred to it. An unbounded veto turns the safety guard into the wedge. Progress is measured,
    // not assumed: any real transition step changes the signature, so only a signature that has not
    // moved in `SAVE_PICKER_TRANSITION_STALL_LIMIT` pumps is treated as abandoned.
    let transition_owned = save_picker_system_rows_transition_owned();
    // A STALLED transition is itself the wedge, not a hint that some other field is wrong. Requiring
    // an additional dirty field before resetting is what made the watchdog useless on 2026-08-11:
    // it declared a stall 77 times and released nothing, because mode/owner/tracked identity all
    // looked clean while `SAVE_PICKER_PATH_EDITOR_RETURN_PENDING_DIALOG` stayed armed forever
    // ("armed no-close parent return reopen ... waiting for native owner disappearance", after which
    // the picker never pumped again). The stuck latch was the only dirty thing, and nothing else
    // could see it. The sanctioned reset clears exactly these latches, so a stall goes straight to it.
    let transition_stalled = transition_owned && picker_transition_is_stalled();
    if transition_owned && !transition_stalled {
        return false;
    }
    let mode_active = SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0;
    let tracked_system = save_picker_system_dialog_identity().map_or(0, |identity| identity.dialog);
    let tracked_action = SAVE_PICKER_ACTION_OBJ.load(Ordering::SeqCst);
    let real_windows_hidden = SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    if !transition_stalled
        && !mode_active
        && !real_windows_hidden
        && tracked_system == 0
        && tracked_action == 0
        && SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) == 0
    {
        // Nothing latched, nothing tracked, no owner, nothing hidden: genuinely idle.
        return false;
    }
    let profile_owner = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let profile_vtable = if profile_owner >= 0x10000 {
        unsafe { safe_read_usize(profile_owner) }.unwrap_or(0)
    } else {
        0
    };
    let expected_profile_vtable = game_module_base()
        .ok()
        .and_then(|base| base.checked_add(PROFILE_LOAD_DIALOG_VTABLE_RVA))
        .unwrap_or(0);
    let owner_dangling = picker_owner_is_dangling(PickerStaleOwnerFacts {
        profile_owner,
        profile_vtable,
        expected_profile_vtable,
        live_owner_authorized: picker_owner_lifetime()
            .current_live_token(profile_owner, profile_vtable, expected_profile_vtable)
            .is_some(),
        owner_zero_resubmit_pending: false,
    });
    let facts = PickerOrphanFacts {
        mode_active,
        profile_owner,
        owner_dangling,
        tracked_system,
        tracked_action,
        real_windows_hidden,
        transition_owned: false,
    };
    if !transition_stalled {
        // A stalled transition has already served its own 600-pump bound, so it acts immediately.
        // Everything else must prove it is not a handoff in progress.
        if !picker_state_is_orphaned(facts) {
            SAVE_PICKER_ORPHAN_PUMPS.store(0, Ordering::SeqCst);
            *SAVE_PICKER_ORPHAN_LAST_FACTS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            return false;
        }
        if !picker_orphan_state_persisted(facts) {
            return false;
        }
    }
    er_telemetry::counters::SAVE_PICKER_DEAD_MODE_RELEASES.fetch_add(1, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_STALE_OWNER_LAST_DIALOG
        .store(profile_owner, Ordering::SeqCst);
    er_telemetry::counters::SAVE_PICKER_STALE_OWNER_LAST_VTABLE
        .store(profile_vtable, Ordering::SeqCst);
    // Restore, do not merely reset. Opening the picker HIDES the real System windows (IngameTop +
    // OptionSetting, which carry the quit-menu rows), and the only un-hide runs inside a
    // `05_010_ProfileSelect` post when the owner clears -- but once the picker window is gone there
    // are no more 05_010 posts, so it never arrives. Measured 2026-08-11: 7 hides, 1 restore. The
    // leftover hidden windows are why backing out landed on the sibling native ProfileSelect (the
    // vanilla per-character Load Game list) instead of the quit menu. `system_quit_restore_real_
    // system_windows` performs the picker reset itself when nothing is hidden, so it is a strict
    // superset of the reset this used to call.
    let reset = match game_module_base() {
        Ok(base) => {
            unsafe { system_quit_restore_real_system_windows(base, "orphaned-picker-state") };
            true
        }
        Err(_) => unsafe { system_quit_reset_profile_select_state("orphaned-picker-state") },
    };
    append_autoload_debug(format_args!(
        "save-picker: released ORPHANED picker state stalled={transition_stalled} mode_active={mode_active} owner=0x{profile_owner:x} vtable=0x{profile_vtable:x} owner_dangling={owner_dangling} tracked_system=0x{tracked_system:x} tracked_action=0x{tracked_action:x} reset={reset}; leftover picker state can only reject every load-source open"
    ));
    reset
}

pub(crate) unsafe fn begin_picker_load_source_open_preflight(
    action_obj: usize,
) -> PickerLoadSourceOpenPreflight {
    // Self-healing invariant: a leaked owner from ANY teardown route would otherwise reject this
    // open (and every future one) forever. Run before the facts are taken so the classification
    // below sees the corrected owner.
    let _ = unsafe { release_dangling_profile_owner() };
    let facts = unsafe { picker_load_source_open_facts(action_obj) };
    let preflight = picker_load_source_open_preflight_with(
        facts,
        || {
            try_begin_picker_initial_open_transaction(|| {
                classify_picker_load_source_open(unsafe {
                    picker_load_source_open_facts(action_obj)
                }) == PickerLoadSourceOpenDecision::Initial
            })
        },
        || unsafe { picker_load_source_open_facts(action_obj) },
    );
    match &preflight {
        PickerLoadSourceOpenPreflightWith::Initial(_) => {}
        PickerLoadSourceOpenPreflightWith::Coalesced(_, facts) => {
            er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_COALESCES
                .fetch_add(1, Ordering::SeqCst);
            record_picker_open_preflight_facts(PICKER_OPEN_SOURCE_COALESCED, *facts);
        }
        PickerLoadSourceOpenPreflightWith::Rejected(facts) => {
            er_telemetry::counters::SAVE_PICKER_OPEN_PREFLIGHT_REJECTIONS
                .fetch_add(1, Ordering::SeqCst);
            record_picker_open_preflight_facts(PICKER_OPEN_SOURCE_REJECTED, *facts);
        }
    }
    preflight
}
