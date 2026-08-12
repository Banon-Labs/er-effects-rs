use super::*;

const SOFTWARE_KEYBOARD_JOB_SIZE: usize = 0x1a8;
const SOFTWARE_KEYBOARD_VALIDATOR_SIZE: usize = 0x70;
const SOFTWARE_KEYBOARD_JOB_CTOR_RVA: u32 = 0x81be30;
const SOFTWARE_KEYBOARD_JOB_CTOR_SIG: &[u8] = &[
    0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x55, 0x56, 0x57, 0x41, 0x56, 0x48, 0x83, 0xec, 0x30,
];
const SOFTWARE_KEYBOARD_RESULT_GATE_RVA: u32 = 0x81d3d0;
const SOFTWARE_KEYBOARD_RESULT_GATE_SIG: &[u8] = &[
    0x4c, 0x89, 0x44, 0x24, 0x18, 0x55, 0x56, 0x57, 0x48, 0x83, 0xec, 0x40,
];
// FUN_14081d220 is the accepted-text continuation queued by FUN_14081d050. Its native
// implementation unconditionally invokes the SoftwareKeyboardJob callback stored at +0x1a0 and
// throws std::bad_function_call when that slot is null. We deliberately provide no unproven MSVC
// std::function object, so this exact continuation is the crash barrier for generation-owned jobs.
const SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_RVA: u32 = 0x81d220;
const SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_SIG: &[u8] = &[
    0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8d, 0x6c, 0x24,
    0xd9,
];
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA: u32 = 0xe70920;
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30];
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA: u32 = 0xe70960;
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30];
const SOFTWARE_KEYBOARD_ENTER_NAME_RVA: u32 = 0xe70c00;
// Ghidra/deobf 1.16.2 entry 0x140e70c00: the leading 0x40 is a valid REX prefix on PUSH RBP.
const SOFTWARE_KEYBOARD_ENTER_NAME_SIG: &[u8] = &[0x40, 0x55, 0x56, 0x57, 0x48, 0x83, 0xec, 0x70];
const SOFTWARE_KEYBOARD_SET_INITIAL_RVA: u32 = 0xe709f0;
const SOFTWARE_KEYBOARD_SET_INITIAL_SIG: &[u8] =
    &[0x40, 0x55, 0x56, 0x57, 0x48, 0x8d, 0x6c, 0x24, 0xb9];
const SOFTWARE_KEYBOARD_SET_MAX_RVA: u32 = 0x2416ee0;
const SOFTWARE_KEYBOARD_SET_MAX_SIG: &[u8] =
    &[0xb8, 0x01, 0x00, 0x00, 0x00, 0x3b, 0xd0, 0x0f, 0x4d, 0xc2];
const GAME_HEAP_ALLOC_SIG: &[u8] = &[0x49, 0x8b, 0x00, 0x4d, 0x8b, 0xc8, 0x4c, 0x8b, 0xc2];
const GLOBAL_MENU_HEAP_ALLOCATOR_RVA: usize = 0x3d87350;

const SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET: usize = 0xd8;
const SOFTWARE_KEYBOARD_JOB_CALLBACK_1A0_OFFSET: usize = 0x1a0;
const SOFTWARE_KEYBOARD_CONTROLLER_RESULT_78_OFFSET: usize = 0x78;
const SOFTWARE_KEYBOARD_CONTROLLER_TEXT_80_OFFSET: usize = 0x80;
const DLSTRING_DATA_08_OFFSET: usize = 0x08;
const DLSTRING_LENGTH_18_OFFSET: usize = 0x18;
const DLSTRING_CAPACITY_20_OFFSET: usize = 0x20;
const SOFTWARE_KEYBOARD_VALIDATOR_MAX_60_OFFSET: usize = 0x60;
const SOFTWARE_KEYBOARD_VALIDATOR_FLAGS_68_OFFSET: usize = 0x68;
const SOFTWARE_KEYBOARD_VALIDATOR_MAX_6C_OFFSET: usize = 0x6c;
const SOFTWARE_KEYBOARD_MAX_PATH_UNITS: usize = 1024;
const MENU_JOB_REFCOUNT_08_OFFSET: usize = 0x08;
const MENU_JOB_STATE_CONTINUE: i32 = 1;
const MENU_JOB_STATE_SUCCESS: i32 = 2;
const MENU_JOB_STATE_FAILED: i32 = 3;
const PATH_EDITOR_WINDOW_STALE_PROFILE_TICKS: usize = 3;

use er_save_picker::PathEditorLifecycleStatus;

const PATH_EDITOR_STATUS_IDLE: PathEditorLifecycleStatus = PathEditorLifecycleStatus::Idle;
const PATH_EDITOR_STATUS_PENDING: PathEditorLifecycleStatus = PathEditorLifecycleStatus::Pending;
const PATH_EDITOR_STATUS_SUBMITTED: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::Submitted;
const PATH_EDITOR_STATUS_NATIVE_ACCEPT: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::NativeAccept;
const PATH_EDITOR_STATUS_NATIVE_CANCEL: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::NativeCancel;
const PATH_EDITOR_STATUS_STALE_RESULT: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::StaleResult;
const PATH_EDITOR_STATUS_IDENTITY_REJECTED: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::IdentityRejected;
const PATH_EDITOR_STATUS_SUBMIT_FAILED: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::SubmitFailed;
const PATH_EDITOR_STATUS_VALIDATION_REJECTED: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::ValidationRejected;
const PATH_EDITOR_STATUS_APPLIED_DIRECTORY: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::AppliedDirectory;
const PATH_EDITOR_STATUS_REBUILD_SCHEDULED: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::RebuildScheduled;
const PATH_EDITOR_STATUS_RESET: PathEditorLifecycleStatus = PathEditorLifecycleStatus::Reset;
const PATH_EDITOR_STATUS_RESET_DEFERRED: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::ResetDeferred;
const PATH_EDITOR_STATUS_RECIPE_UNAVAILABLE: PathEditorLifecycleStatus =
    PathEditorLifecycleStatus::RecipeUnavailable;

const TEXT_INPUT_RESOURCE: [u16; 17] = [
    b'0' as u16,
    b'2' as u16,
    b'_' as u16,
    b'9' as u16,
    b'9' as u16,
    b'0' as u16,
    b'_' as u16,
    b'T' as u16,
    b'e' as u16,
    b'x' as u16,
    b't' as u16,
    b'I' as u16,
    b'n' as u16,
    b'p' as u16,
    b'u' as u16,
    b't' as u16,
    0,
];

#[repr(C)]
struct SoftwareKeyboardConfig {
    max_units: u32,
    mode: u8,
    padding: [u8; 3],
    resource: *const u16,
}

struct SoftwareKeyboardRecipe {
    ctor: usize,
    validator_init: usize,
    validator_dtor: usize,
    enter_name: usize,
    set_initial: usize,
    set_max: usize,
    heap_alloc: usize,
    queue_ready: usize,
    submit: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathEditorSubmitDisposition {
    Submitted,
    Retryable,
    RecipeUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathEditorSubmitEvent {
    Attempt,
    Success,
    Failure,
    RecipeUnavailableRejection,
}

#[derive(Debug)]
enum PathEditorOutcome {
    Accepted(String),
    Cancelled,
    TextUnreadable,
}

static SOFTWARE_KEYBOARD_RECIPE: OnceLock<Option<SoftwareKeyboardRecipe>> = OnceLock::new();
static SOFTWARE_KEYBOARD_RECIPE_UNAVAILABLE_LOGGED: AtomicUsize = AtomicUsize::new(0);
static SOFTWARE_KEYBOARD_RESULT_GATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_INSTALLED: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathEditorWindowDisposition {
    Unowned,
    Live {
        first_observation: bool,
    },
    TerminalCancelled {
        job: usize,
    },
    StoppedCancelled {
        job: usize,
        window: usize,
        stale_ticks: usize,
    },
}

struct PathEditorWindowTracker {
    window: AtomicUsize,
    generation: std::sync::atomic::AtomicU64,
    job: AtomicUsize,
    last_profile_tick: AtomicUsize,
}

impl PathEditorWindowTracker {
    const fn new() -> Self {
        Self {
            window: AtomicUsize::new(0),
            generation: std::sync::atomic::AtomicU64::new(0),
            job: AtomicUsize::new(0),
            last_profile_tick: AtomicUsize::new(0),
        }
    }

    fn reset(&self, profile_tick: usize) {
        // Window is the publication flag: clear it before invalidating the stored provenance.
        self.window.store(0, Ordering::SeqCst);
        self.generation.store(0, Ordering::SeqCst);
        self.job.store(0, Ordering::SeqCst);
        self.last_profile_tick.store(profile_tick, Ordering::SeqCst);
    }

    fn note_state(
        &self,
        window: usize,
        state: i32,
        profile_tick: usize,
        active: Option<er_save_picker::PathEditorActiveProvenance>,
        cancel_active: impl FnOnce(er_save_picker::PathEditorActiveProvenance) -> Option<usize>,
    ) -> PathEditorWindowDisposition {
        if window == 0 {
            return PathEditorWindowDisposition::Unowned;
        }
        if path_editor_window_is_live(state) {
            let Some(active) = active else {
                return PathEditorWindowDisposition::Unowned;
            };
            let previous_window = self.window.load(Ordering::SeqCst);
            let previous_generation = self.generation.load(Ordering::SeqCst);
            let previous_job = self.job.load(Ordering::SeqCst);
            if previous_window != 0
                && (previous_window != window
                    || previous_generation != active.generation
                    || previous_job != active.job)
            {
                return PathEditorWindowDisposition::Unowned;
            }
            // Publish provenance before the nonzero window flag.
            self.generation.store(active.generation, Ordering::SeqCst);
            self.job.store(active.job, Ordering::SeqCst);
            self.window.store(window, Ordering::SeqCst);
            self.last_profile_tick.store(profile_tick, Ordering::SeqCst);
            return PathEditorWindowDisposition::Live {
                first_observation: previous_window == 0,
            };
        }

        // A terminal event may retire only a window that was observed live and bound to one exact
        // generation/job pair. Never infer ownership from the coordinator's current active address.
        let tracked_window = self.window.load(Ordering::SeqCst);
        if tracked_window == 0 || tracked_window != window {
            return PathEditorWindowDisposition::Unowned;
        }
        let expected = er_save_picker::PathEditorActiveProvenance {
            generation: self.generation.load(Ordering::SeqCst),
            job: self.job.load(Ordering::SeqCst),
        };
        if expected.generation == 0 || expected.job == 0 || active != Some(expected) {
            return PathEditorWindowDisposition::Unowned;
        }
        if self
            .window
            .compare_exchange(tracked_window, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return PathEditorWindowDisposition::Unowned;
        }
        self.generation.store(0, Ordering::SeqCst);
        self.job.store(0, Ordering::SeqCst);
        cancel_active(expected)
            .map(|job| PathEditorWindowDisposition::TerminalCancelled { job })
            .unwrap_or(PathEditorWindowDisposition::Unowned)
    }

    fn expire_stopped(
        &self,
        profile_tick: usize,
        active: Option<er_save_picker::PathEditorActiveProvenance>,
        cancel_active: impl FnOnce(er_save_picker::PathEditorActiveProvenance) -> Option<usize>,
    ) -> PathEditorWindowDisposition {
        let window = self.window.load(Ordering::SeqCst);
        if window == 0 {
            return PathEditorWindowDisposition::Unowned;
        }
        let expected = er_save_picker::PathEditorActiveProvenance {
            generation: self.generation.load(Ordering::SeqCst),
            job: self.job.load(Ordering::SeqCst),
        };
        if expected.generation == 0 || expected.job == 0 || active != Some(expected) {
            return PathEditorWindowDisposition::Unowned;
        }
        let last = self.last_profile_tick.load(Ordering::SeqCst);
        let stale_ticks = profile_tick.saturating_sub(last);
        if stale_ticks < PATH_EDITOR_WINDOW_STALE_PROFILE_TICKS
            || self
                .window
                .compare_exchange(window, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return PathEditorWindowDisposition::Unowned;
        }
        self.generation.store(0, Ordering::SeqCst);
        self.job.store(0, Ordering::SeqCst);
        cancel_active(expected)
            .map(|job| PathEditorWindowDisposition::StoppedCancelled {
                job,
                window,
                stale_ticks,
            })
            .unwrap_or(PathEditorWindowDisposition::Unowned)
    }
}

static SAVE_PICKER_PATH_EDITOR_WINDOW: PathEditorWindowTracker = PathEditorWindowTracker::new();

#[derive(Clone, Copy)]
struct PathEditorTelemetryPublisher;

impl er_save_picker::PathEditorLifecyclePublisher for PathEditorTelemetryPublisher {
    fn publish(&self, snapshot: er_save_picker::PathEditorLifecycleSnapshot) {
        SAVE_PICKER_PATH_EDITOR_PENDING.store(snapshot.pending as usize, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_GENERATION.store(snapshot.generation as usize, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_SUBMIT_LEASE_ACTIVE
            .store(snapshot.submit_lease_active as usize, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_LAST_STATUS.store(snapshot.status as usize, Ordering::SeqCst);
    }

    fn invariant_violation(&self) {
        SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
    }
}

type RuntimePathEditorCoordinator =
    er_save_picker::PathEditorCoordinator<PathEditorOutcome, PathEditorTelemetryPublisher>;
static SAVE_PICKER_PATH_EDITOR_LIFECYCLE: OnceLock<RuntimePathEditorCoordinator> = OnceLock::new();

fn path_editor_lifecycle() -> &'static RuntimePathEditorCoordinator {
    SAVE_PICKER_PATH_EDITOR_LIFECYCLE
        .get_or_init(|| RuntimePathEditorCoordinator::new(PathEditorTelemetryPublisher))
}

pub(crate) struct PathEditorResetLeaseGuard {
    _guard: er_save_picker::PathEditorResetGuard<
        'static,
        PathEditorOutcome,
        PathEditorTelemetryPublisher,
    >,
}

fn current_picker_identity() -> er_save_picker::PathEditorPickerIdentity {
    er_save_picker::PathEditorPickerIdentity {
        picker_mode_active: SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
        current_dialog: SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst),
    }
}

fn path_editor_window_is_live(state: i32) -> bool {
    // A newly-created MenuWindow begins at zero before its first controller update. Continue is 1;
    // only Success/Failed are terminal. Treating zero as terminal releases the editor during its
    // construction frame and can rebuild ProfileSelect against half-bound child components.
    state == 0 || state == MENU_JOB_STATE_CONTINUE
}

fn cancel_active_path_editor_for_window_lifetime(
    expected: er_save_picker::PathEditorActiveProvenance,
) -> Option<usize> {
    let ownership = path_editor_lifecycle().record_active_result(
        expected,
        PathEditorOutcome::Cancelled,
        PATH_EDITOR_STATUS_NATIVE_CANCEL,
    )?;
    if ownership != er_save_picker::PathEditorResultOwnership::Current {
        SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    SAVE_PICKER_PATH_EDITOR_NATIVE_CANCELS.fetch_add(1, Ordering::SeqCst);
    Some(expected.job)
}

/// Called from the owned 02_990 MenuWindowJob::Run post-hook. A terminal MenuWindow result means
/// its SceneObjProxy lifetime is ending, so retire the exact active SoftwareKeyboard job before any
/// later picker-pump work can retain or act on that native session.
pub(crate) fn save_picker_note_path_editor_window_state(window: usize, state: i32) -> bool {
    let profile_tick =
        er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst);
    let disposition = SAVE_PICKER_PATH_EDITOR_WINDOW.note_state(
        window,
        state,
        profile_tick,
        path_editor_lifecycle().active_provenance(),
        cancel_active_path_editor_for_window_lifetime,
    );
    match disposition {
        PathEditorWindowDisposition::Live { first_observation } => {
            if first_observation {
                append_autoload_debug(format_args!(
                    "save-picker-path: observed owned 02_990 MenuWindow window=0x{window:x}; native keyboard window lifetime tracking armed"
                ));
            }
            true
        }
        PathEditorWindowDisposition::TerminalCancelled { job } => {
            append_autoload_debug(format_args!(
                "save-picker-path: 02_990 MenuWindow became terminal state={state} window=0x{window:x}; retired job=0x{job:x} as cancelled before proxy teardown"
            ));
            false
        }
        PathEditorWindowDisposition::Unowned
        | PathEditorWindowDisposition::StoppedCancelled { .. } => false,
    }
}

fn expire_stopped_path_editor_window() {
    let profile_tick =
        er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst);
    if let PathEditorWindowDisposition::StoppedCancelled {
        job,
        window,
        stale_ticks,
    } = SAVE_PICKER_PATH_EDITOR_WINDOW.expire_stopped(
        profile_tick,
        path_editor_lifecycle().active_provenance(),
        cancel_active_path_editor_for_window_lifetime,
    ) {
        append_autoload_debug(format_args!(
            "save-picker-path: 02_990 MenuWindow stopped running for {stale_ticks} ProfileSelect ticks; retired stale job=0x{job:x} window=0x{window:x} as cancelled before any later native controller access"
        ));
    }
}

fn path_editor_set_status(status: PathEditorLifecycleStatus) {
    path_editor_lifecycle().set_status(status);
}

fn path_editor_named_rejection(reason: &str) {
    SAVE_PICKER_PATH_EDITOR_SUBMIT_REJECTIONS.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-path: lifecycle rejection={reason}; pending work cleared and stale results fail closed"
    ));
}

fn resolve_software_keyboard_recipe_with(
    mut verify: impl FnMut(u32, &[u8], &'static str) -> Option<usize>,
    mut resolve: impl FnMut(u32) -> Option<usize>,
) -> Option<SoftwareKeyboardRecipe> {
    Some(SoftwareKeyboardRecipe {
        ctor: verify(
            SOFTWARE_KEYBOARD_JOB_CTOR_RVA,
            SOFTWARE_KEYBOARD_JOB_CTOR_SIG,
            "SoftwareKeyboardJob ctor",
        )?,
        validator_init: verify(
            SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA,
            SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG,
            "SoftwareKeyboard validator init",
        )?,
        validator_dtor: verify(
            SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA,
            SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG,
            "SoftwareKeyboard validator dtor",
        )?,
        enter_name: verify(
            SOFTWARE_KEYBOARD_ENTER_NAME_RVA,
            SOFTWARE_KEYBOARD_ENTER_NAME_SIG,
            "SoftwareKeyboard EnterName preset",
        )?,
        set_initial: verify(
            SOFTWARE_KEYBOARD_SET_INITIAL_RVA,
            SOFTWARE_KEYBOARD_SET_INITIAL_SIG,
            "SoftwareKeyboard initial text setter",
        )?,
        set_max: verify(
            SOFTWARE_KEYBOARD_SET_MAX_RVA,
            SOFTWARE_KEYBOARD_SET_MAX_SIG,
            "SoftwareKeyboard max-length setter",
        )?,
        heap_alloc: verify(
            GAME_HEAP_ALLOC_RVA as u32,
            GAME_HEAP_ALLOC_SIG,
            "game heap allocator",
        )?,
        queue_ready: resolve(MENU_JOB_QUEUE_READY_RVA)?,
        submit: resolve(MENU_JOB_SUBMIT_RVA)?,
    })
}

fn software_keyboard_recipe() -> Option<&'static SoftwareKeyboardRecipe> {
    SOFTWARE_KEYBOARD_RECIPE
        .get_or_init(|| {
            resolve_software_keyboard_recipe_with(save_flow_verify_rva, |rva| game_rva(rva).ok())
        })
        .as_ref()
}

fn dispatch_recipe_submit_with<R>(
    recipe: Option<&R>,
    queue_ready: impl FnOnce(&R) -> bool,
    submit: impl FnOnce(&R) -> bool,
) -> PathEditorSubmitDisposition {
    let Some(recipe) = recipe else {
        return PathEditorSubmitDisposition::RecipeUnavailable;
    };
    if !queue_ready(recipe) {
        return PathEditorSubmitDisposition::Retryable;
    }
    if submit(recipe) {
        PathEditorSubmitDisposition::Submitted
    } else {
        PathEditorSubmitDisposition::Retryable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SoftwareKeyboardCallbackProvenance {
    /// The lifecycle owns this address and the constructor's exact null callback invariant holds.
    OwnedNull,
    /// A delayed null-callback job outlived the bounded tombstone ring. Native acceptance would
    /// throw, so it is completed/suppressed without publishing text into any current generation.
    OrphanedNull,
    /// A valid non-null callback proves this is a native/foreign allocation, even when its address
    /// reuses one of our retired tombstones.
    ForeignNonNull,
    /// A lifecycle-owned address whose callback slot cannot be read is unsafe to forward or complete.
    UnreadableOwned,
    /// No lifecycle ownership and no readable null invariant: preserve native behavior.
    UnreadableForeign,
}

fn software_keyboard_callback_provenance(
    address_recognized: bool,
    callback: Option<usize>,
) -> SoftwareKeyboardCallbackProvenance {
    match (address_recognized, callback) {
        (true, Some(0)) => SoftwareKeyboardCallbackProvenance::OwnedNull,
        (false, Some(0)) => SoftwareKeyboardCallbackProvenance::OrphanedNull,
        (_, Some(_)) => SoftwareKeyboardCallbackProvenance::ForeignNonNull,
        (true, None) => SoftwareKeyboardCallbackProvenance::UnreadableOwned,
        (false, None) => SoftwareKeyboardCallbackProvenance::UnreadableForeign,
    }
}

unsafe fn software_keyboard_job_callback_provenance(
    job: usize,
    address_recognized: bool,
) -> (Option<usize>, SoftwareKeyboardCallbackProvenance) {
    let callback = unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CALLBACK_1A0_OFFSET) };
    (
        callback,
        software_keyboard_callback_provenance(address_recognized, callback),
    )
}

impl SoftwareKeyboardCallbackProvenance {
    fn suppresses_success(self) -> bool {
        matches!(
            self,
            Self::OwnedNull | Self::OrphanedNull | Self::UnreadableOwned
        )
    }

    fn records_owned_result(self) -> bool {
        self == Self::OwnedNull
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SoftwareKeyboardSuccessDisposition {
    Current,
    Stale,
    Orphaned,
    Incomplete,
}

/// Complete native state before publishing a lifecycle outcome. Null-callback jobs with evicted
/// tombstones are completed without publication; unreadable callback provenance and invalid result
/// storage are suppressed without fabricating success.
fn consume_software_keyboard_success_with(
    provenance: SoftwareKeyboardCallbackProvenance,
    complete: impl FnOnce() -> bool,
    record: impl FnOnce() -> er_save_picker::PathEditorResultOwnership,
) -> SoftwareKeyboardSuccessDisposition {
    if provenance == SoftwareKeyboardCallbackProvenance::UnreadableOwned {
        return SoftwareKeyboardSuccessDisposition::Incomplete;
    }
    if !complete() {
        return SoftwareKeyboardSuccessDisposition::Incomplete;
    }
    if provenance == SoftwareKeyboardCallbackProvenance::OrphanedNull {
        return SoftwareKeyboardSuccessDisposition::Orphaned;
    }
    debug_assert!(provenance.records_owned_result());
    match record() {
        er_save_picker::PathEditorResultOwnership::Current => {
            SoftwareKeyboardSuccessDisposition::Current
        }
        er_save_picker::PathEditorResultOwnership::StaleOwned
        | er_save_picker::PathEditorResultOwnership::Foreign => {
            SoftwareKeyboardSuccessDisposition::Stale
        }
    }
}

/// Shared production/test routing for FUN_14081d3d0. Only a success with proven null-callback
/// provenance is suppressed; cancellation forwards native exactly once and is recorded only for
/// exact lifecycle-owned/null-callback jobs. Non-null same-address reuse is foreign and forwards.
fn dispatch_software_keyboard_result_gate_with<R>(
    provenance: SoftwareKeyboardCallbackProvenance,
    keyboard_state: i32,
    accept: impl FnOnce() -> R,
    forward: impl FnOnce() -> (R, i32),
    cancel: impl FnOnce(),
) -> R {
    if provenance.suppresses_success() && keyboard_state == MENU_JOB_STATE_SUCCESS {
        return accept();
    }
    let (ret, result_state) = forward();
    if provenance.records_owned_result() && result_state == MENU_JOB_STATE_FAILED {
        cancel();
    }
    ret
}

/// Shared production/test routing for FUN_14081d220. Proven owned, orphaned-null, and unreadable
/// owned jobs suppress the throwing original. Valid non-null callbacks and unrecognized unreadable
/// jobs preserve native behavior exactly once.
fn dispatch_software_keyboard_accept_continuation_with<R>(
    provenance: SoftwareKeyboardCallbackProvenance,
    consume_suppressed: impl FnOnce(),
    forward_foreign: impl FnOnce() -> R,
    suppressed_return: R,
) -> R {
    if !provenance.suppresses_success() {
        return forward_foreign();
    }
    consume_suppressed();
    suppressed_return
}

fn pump_path_editor_submit_with<T, P: er_save_picker::PathEditorLifecyclePublisher>(
    coordinator: &er_save_picker::PathEditorCoordinator<T, P>,
    identity: er_save_picker::PathEditorPickerIdentity,
    submit: impl FnOnce(
        er_save_picker::PathEditorRequestTicket,
        &er_save_picker::PathEditorCoordinator<T, P>,
    ) -> PathEditorSubmitDisposition,
    mut observe: impl FnMut(PathEditorSubmitEvent),
) -> Result<Option<PathEditorSubmitDisposition>, er_save_picker::PathEditorLifecycleRejection> {
    let nested = coordinator.with_submit(identity, |ticket, coordinator| {
        observe(PathEditorSubmitEvent::Attempt);
        let disposition = submit(ticket, coordinator);
        match disposition {
            PathEditorSubmitDisposition::Submitted => observe(PathEditorSubmitEvent::Success),
            PathEditorSubmitDisposition::Retryable => {
                coordinator.set_status(PATH_EDITOR_STATUS_SUBMIT_FAILED);
                observe(PathEditorSubmitEvent::Failure);
            }
            PathEditorSubmitDisposition::RecipeUnavailable => {
                coordinator.reject_pending_submit(ticket, PATH_EDITOR_STATUS_RECIPE_UNAVAILABLE)?;
                observe(PathEditorSubmitEvent::Failure);
                observe(PathEditorSubmitEvent::RecipeUnavailableRejection);
            }
        }
        Ok(disposition)
    })?;
    nested.transpose()
}

fn record_path_editor_submit_event(event: PathEditorSubmitEvent) {
    match event {
        PathEditorSubmitEvent::Attempt => {
            SAVE_PICKER_PATH_EDITOR_SUBMIT_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        }
        PathEditorSubmitEvent::Success => {
            SAVE_PICKER_PATH_EDITOR_SUBMIT_SUCCESSES.fetch_add(1, Ordering::SeqCst);
        }
        PathEditorSubmitEvent::Failure => {
            SAVE_PICKER_PATH_EDITOR_SUBMIT_FAILURES.fetch_add(1, Ordering::SeqCst);
        }
        PathEditorSubmitEvent::RecipeUnavailableRejection => {
            SAVE_PICKER_PATH_EDITOR_SUBMIT_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            if SOFTWARE_KEYBOARD_RECIPE_UNAVAILABLE_LOGGED
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                append_autoload_debug(format_args!(
                    "save-picker-path: terminal submit rejection=software-keyboard-recipe-unavailable; pending request retired after cached verifier failure"
                ));
            }
        }
    }
}

fn install_software_keyboard_result_gate_hook() -> bool {
    if SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED.load(Ordering::SeqCst) == 1 {
        return true;
    }
    let Some(address) = save_flow_verify_rva(
        SOFTWARE_KEYBOARD_RESULT_GATE_RVA,
        SOFTWARE_KEYBOARD_RESULT_GATE_SIG,
        "SoftwareKeyboard accepted/cancel gate",
    ) else {
        return false;
    };
    mh_install_hook_once(
        &SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED,
        0,
        1,
        address,
        software_keyboard_result_gate_hook as *mut c_void,
        &SOFTWARE_KEYBOARD_RESULT_GATE_ORIG,
        "SoftwareKeyboard path result gate",
    );
    SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED.load(Ordering::SeqCst) == 1
}

fn install_software_keyboard_accept_continuation_hook() -> bool {
    if SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_INSTALLED.load(Ordering::SeqCst) == 1 {
        return true;
    }
    let Some(address) = save_flow_verify_rva(
        SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_RVA,
        SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_SIG,
        "SoftwareKeyboard accepted callback continuation",
    ) else {
        return false;
    };
    mh_install_hook_once(
        &SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_INSTALLED,
        0,
        1,
        address,
        software_keyboard_accept_continuation_hook as *mut c_void,
        &SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_ORIG,
        "SoftwareKeyboard path accepted callback continuation",
    );
    SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_INSTALLED.load(Ordering::SeqCst) == 1
}

fn install_software_keyboard_result_hooks() -> bool {
    // Install the null-callback crash barrier first. A result-gate-only install is insufficient on
    // the native callback route through FUN_14081d5c0, which bypasses FUN_14081d3d0 entirely.
    install_software_keyboard_accept_continuation_hook()
        && install_software_keyboard_result_gate_hook()
}

unsafe fn software_keyboard_text(job: usize) -> Option<String> {
    let controller = unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET) }?;
    if controller == 0 || controller == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let text = controller + SOFTWARE_KEYBOARD_CONTROLLER_TEXT_80_OFFSET;
    let length = unsafe { safe_read_usize(text + DLSTRING_LENGTH_18_OFFSET) }?;
    let capacity = unsafe { safe_read_usize(text + DLSTRING_CAPACITY_20_OFFSET) }?;
    if length > SOFTWARE_KEYBOARD_MAX_PATH_UNITS {
        return None;
    }
    let data = if capacity > 7 {
        unsafe { safe_read_usize(text + DLSTRING_DATA_08_OFFSET) }?
    } else {
        text + DLSTRING_DATA_08_OFFSET
    };
    if data == 0 || data == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let mut units = Vec::with_capacity(length);
    for index in 0..length {
        units.push(unsafe { safe_read_u16(data + index * 2) }?);
    }
    String::from_utf16(&units).ok()
}

fn record_path_editor_result(
    job: usize,
    outcome: PathEditorOutcome,
    current_status: PathEditorLifecycleStatus,
) -> er_save_picker::PathEditorResultOwnership {
    let ownership = path_editor_lifecycle().record_result(job, outcome, current_status);
    match ownership {
        er_save_picker::PathEditorResultOwnership::Current => {}
        er_save_picker::PathEditorResultOwnership::StaleOwned => {
            SAVE_PICKER_PATH_EDITOR_STALE_RESULTS.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-path: stale/duplicate native result rejected job=0x{job:x}"
            ));
        }
        er_save_picker::PathEditorResultOwnership::Foreign => {
            SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-path: callback provenance changed before result publication job=0x{job:x}"
            ));
        }
    }
    ownership
}

fn software_keyboard_writable_range(address: usize, size: usize) -> bool {
    use windows::Win32::System::Memory::{
        MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS,
        PAGE_READWRITE, PAGE_WRITECOPY,
    };

    if address < 0x10000 || size == 0 {
        return false;
    }
    let Some(end) = address.checked_add(size) else {
        return false;
    };
    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    let queried = unsafe {
        VirtualQuery(
            Some(address as *const c_void),
            &mut mbi,
            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    let Some(region_end) = (mbi.BaseAddress as usize).checked_add(mbi.RegionSize) else {
        return false;
    };
    let writable =
        PAGE_READWRITE.0 | PAGE_WRITECOPY.0 | PAGE_EXECUTE_READWRITE.0 | PAGE_EXECUTE_WRITECOPY.0;
    queried != 0
        && mbi.State == MEM_COMMIT
        && end <= region_end
        && mbi.Protect.0 & (PAGE_GUARD.0 | PAGE_NOACCESS.0) == 0
        && mbi.Protect.0 & writable != 0
}

unsafe fn complete_software_keyboard_job_success(result: usize, time: usize) -> bool {
    if result % core::mem::align_of::<i32>() != 0
        || time % core::mem::align_of::<usize>() != 0
        || !software_keyboard_writable_range(result, 8)
        || !software_keyboard_writable_range(time, core::mem::size_of::<usize>())
    {
        return false;
    }
    let Ok(base) = game_module_base() else {
        return false;
    };
    unsafe {
        // MenuJobResult::SetResult(result, Success, 0), byte-for-byte equivalent to 0x1407a91e0.
        *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
        *((result + 4) as *mut i32) = 0;
        // FUN_14081d220's common return leaves FD4TimeTemplate<float>'s final vtable installed.
        *(time as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA;
    }
    true
}

unsafe extern "system" fn software_keyboard_result_gate_hook(
    job: usize,
    result: usize,
    time: usize,
) -> usize {
    let original_addr = SOFTWARE_KEYBOARD_RESULT_GATE_ORIG.load(Ordering::SeqCst);
    if original_addr == HOOK_ORIGINAL_UNSET {
        return result;
    }
    let original: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original_addr) };
    let address_recognized = path_editor_lifecycle().recognizes_job(job);
    let (callback, provenance) =
        unsafe { software_keyboard_job_callback_provenance(job, address_recognized) };
    let controller =
        unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET) }.unwrap_or(0);
    let keyboard_state = if controller != 0 && controller != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_i32(controller + SOFTWARE_KEYBOARD_CONTROLLER_RESULT_78_OFFSET) }
            .unwrap_or(0)
    } else {
        0
    };
    if provenance.suppresses_success() {
        SAVE_PICKER_PATH_EDITOR_RESULT_GATE_OWNED_HITS.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-path: result gate entered suppressed job=0x{job:x} controller=0x{controller:x} state={keyboard_state} provenance={provenance:?} callback_1a0={callback:?}"
        ));
    }
    dispatch_software_keyboard_result_gate_with(
        provenance,
        keyboard_state,
        || {
            let disposition = consume_software_keyboard_success_with(
                provenance,
                || unsafe { complete_software_keyboard_job_success(result, time) },
                || {
                    let outcome = match unsafe { software_keyboard_text(job) } {
                        Some(text) => PathEditorOutcome::Accepted(text),
                        None => PathEditorOutcome::TextUnreadable,
                    };
                    record_path_editor_result(job, outcome, PATH_EDITOR_STATUS_NATIVE_ACCEPT)
                },
            );
            if disposition == SoftwareKeyboardSuccessDisposition::Current {
                SAVE_PICKER_PATH_EDITOR_NATIVE_ACCEPTS.fetch_add(1, Ordering::SeqCst);
            }
            if matches!(
                disposition,
                SoftwareKeyboardSuccessDisposition::Incomplete
                    | SoftwareKeyboardSuccessDisposition::Orphaned
            ) {
                SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS
                    .fetch_add(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "save-picker-path: result gate suppressed acceptance job=0x{job:x} provenance={provenance:?} disposition={disposition:?}; completion precedes any lifecycle publication"
            ));
            result
        },
        || {
            let ret = unsafe { original(job, result, time) };
            let result_state = unsafe { safe_read_i32(result) }.unwrap_or(0);
            (ret, result_state)
        },
        || {
            let ownership = record_path_editor_result(
                job,
                PathEditorOutcome::Cancelled,
                PATH_EDITOR_STATUS_NATIVE_CANCEL,
            );
            if ownership == er_save_picker::PathEditorResultOwnership::Current {
                SAVE_PICKER_PATH_EDITOR_NATIVE_CANCELS.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-picker-path: result gate consumed native SoftwareKeyboard cancellation job=0x{job:x}; model remains unchanged"
                ));
            }
        },
    )
}

unsafe extern "system" fn software_keyboard_accept_continuation_hook(
    job: usize,
    result: usize,
    time: usize,
) -> usize {
    let original_addr = SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_ORIG.load(Ordering::SeqCst);
    if original_addr == HOOK_ORIGINAL_UNSET {
        return result;
    }
    let original: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original_addr) };
    let address_recognized = path_editor_lifecycle().recognizes_job(job);
    let (callback, provenance) =
        unsafe { software_keyboard_job_callback_provenance(job, address_recognized) };
    dispatch_software_keyboard_accept_continuation_with(
        provenance,
        || {
            SAVE_PICKER_PATH_EDITOR_ACCEPT_CONTINUATION_OWNED_HITS.fetch_add(1, Ordering::SeqCst);
            let disposition = consume_software_keyboard_success_with(
                provenance,
                || unsafe { complete_software_keyboard_job_success(result, time) },
                || {
                    let outcome = match unsafe { software_keyboard_text(job) } {
                        Some(text) => PathEditorOutcome::Accepted(text),
                        None => PathEditorOutcome::TextUnreadable,
                    };
                    record_path_editor_result(job, outcome, PATH_EDITOR_STATUS_NATIVE_ACCEPT)
                },
            );
            if disposition == SoftwareKeyboardSuccessDisposition::Current {
                SAVE_PICKER_PATH_EDITOR_NATIVE_ACCEPTS.fetch_add(1, Ordering::SeqCst);
            }
            if matches!(
                disposition,
                SoftwareKeyboardSuccessDisposition::Incomplete
                    | SoftwareKeyboardSuccessDisposition::Orphaned
            ) {
                SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS
                    .fetch_add(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "save-picker-path: accepted callback continuation suppressed job=0x{job:x} provenance={provenance:?} callback_1a0={callback:?} disposition={disposition:?}; throwing native original unreachable and completion precedes publication"
            ));
        },
        || unsafe { original(job, result, time) },
        result,
    )
}

pub(crate) fn save_picker_request_path_editor(dialog: usize) {
    let identity = current_picker_identity();
    let result = path_editor_lifecycle().request(identity, dialog);
    match result {
        Ok(_) => {}
        Err(reason) => path_editor_named_rejection(match reason {
            er_save_picker::PathEditorLifecycleRejection::InvalidDialog => "request-invalid-dialog",
            er_save_picker::PathEditorLifecycleRejection::IdentityMismatch => {
                "request-dialog-identity-mismatch"
            }
            er_save_picker::PathEditorLifecycleRejection::Busy => "request-while-busy",
            er_save_picker::PathEditorLifecycleRejection::RequestMismatch => {
                "request-generation-mismatch"
            }
            er_save_picker::PathEditorLifecycleRejection::InvalidJob => "request-invalid-job",
        }),
    }
}

pub(crate) fn save_picker_path_editor_begin_reset(
    source: &str,
) -> Option<PathEditorResetLeaseGuard> {
    match path_editor_lifecycle().begin_reset(current_picker_identity()) {
        er_save_picker::PathEditorResetStart::DeferredForSubmit
        | er_save_picker::PathEditorResetStart::DeferredForReset
        | er_save_picker::PathEditorResetStart::OwnedCloseMustDrain(_) => {
            SAVE_PICKER_PATH_EDITOR_RESET_DEFERRED.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-path: reset/close deferred source={source}; submit/reset ownership or an exact deferred-close ticket must drain first"
            ));
            None
        }
        er_save_picker::PathEditorResetStart::Acquired {
            guard,
            invalidated,
            cancelled_close,
        } => {
            SAVE_PICKER_PATH_EDITOR_WINDOW.reset(
                er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
            );
            if cancelled_close.is_some() {
                SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_CANCELS.fetch_add(1, Ordering::SeqCst);
            }
            if invalidated {
                SAVE_PICKER_PATH_EDITOR_SUBMIT_REJECTIONS.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-picker-path: reset invalidated pending/active/result ownership source={source} generation={}",
                    path_editor_lifecycle().snapshot().generation
                ));
            }
            Some(PathEditorResetLeaseGuard { _guard: guard })
        }
    }
}

pub(crate) fn save_picker_path_editor_close_with(
    dialog: usize,
    close: impl FnOnce(usize) -> bool,
) -> er_save_picker::PathEditorCloseDisposition {
    path_editor_lifecycle().close_with(current_picker_identity(), dialog, close)
}

pub(crate) fn save_picker_path_editor_retry_deferred_close(
    close: impl FnOnce(usize) -> bool,
) -> er_save_picker::PathEditorDeferredCloseDisposition {
    path_editor_lifecycle().retry_deferred_close(current_picker_identity(), close)
}

pub(crate) fn save_picker_path_editor_reset_active() -> bool {
    path_editor_lifecycle().snapshot().reset_lease_active
}

pub(crate) fn save_picker_path_editor_deferred_close_pending() -> bool {
    path_editor_lifecycle().snapshot().deferred_close.is_some()
}

pub(crate) fn save_picker_path_editor_deferred_close_ticket()
-> Option<er_save_picker::PathEditorDeferredCloseTicket> {
    path_editor_lifecycle().snapshot().deferred_close
}

/// Modal ownership barrier for ProfileSelect presentation changes. Pointer motion and queued live
/// editor changes behind 02_990 must not close or replace the picker until the exact keyboard
/// request/job/window generation has retired.
pub(crate) fn save_picker_path_editor_blocks_profile_refresh() -> bool {
    let snapshot = path_editor_lifecycle().snapshot();
    snapshot.pending
        || snapshot.submit_lease_active
        || snapshot.reset_lease_active
        || snapshot.deferred_close.is_some()
        || path_editor_lifecycle().active_provenance().is_some()
        || SAVE_PICKER_PATH_EDITOR_WINDOW.window.load(Ordering::SeqCst) != 0
}

/// Exact generation to preserve when the owning native list proves the parent ProfileSelect has
/// disappeared before the SoftwareKeyboard result bridge consumed its terminal outcome.
pub(crate) fn save_picker_path_editor_ticket_matches_current_owner(
    ticket: er_save_picker::PathEditorRequestTicket,
) -> bool {
    let snapshot = path_editor_lifecycle().snapshot();
    snapshot.owner_dialog == ticket.dialog && snapshot.generation == ticket.generation
}

pub(crate) fn save_picker_path_editor_ticket_for_absent_parent(
    profile: usize,
) -> Option<er_save_picker::PathEditorRequestTicket> {
    let snapshot = path_editor_lifecycle().snapshot();
    (profile != 0
        && snapshot.owner_dialog == profile
        && snapshot.generation != 0
        && save_picker_path_editor_blocks_profile_refresh())
    .then_some(er_save_picker::PathEditorRequestTicket {
        dialog: profile,
        generation: snapshot.generation,
    })
}

pub(crate) fn apply_picker_owner_publication_now(
    request: PickerOwnerPublicationRequest,
) -> PickerOwnerApplyResult {
    let apply_compare = |expected, new_dialog| match path_editor_lifecycle()
        .publish_owner_if_current(
            SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
            expected,
            new_dialog,
            |expected, owner| {
                SYSTEM_QUIT_PROFILE_SELECT_WINDOW.compare_exchange(
                    expected,
                    owner,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
            },
        ) {
        er_save_picker::PathEditorOwnerComparePublication::Published {
            result: previous,
            cancelled_close,
        } => PickerOwnerApplyResult::Published(PickerOwnerAppliedPublication {
            previous,
            cancelled_close,
            lifecycle_generation: path_editor_lifecycle().snapshot().generation,
        }),
        er_save_picker::PathEditorOwnerComparePublication::Stale { actual } => {
            PickerOwnerApplyResult::Stale { actual }
        }
    };
    match request {
        PickerOwnerPublicationRequest::Set { new_dialog, .. } => {
            let current_dialog = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            let er_save_picker::PathEditorOwnerPublication::Published {
                result: previous,
                cancelled_close,
                generation,
            } = path_editor_lifecycle().publish_owner(
                SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
                current_dialog,
                new_dialog,
                |owner| SYSTEM_QUIT_PROFILE_SELECT_WINDOW.swap(owner, Ordering::SeqCst),
            );
            if new_dialog != 0 {
                save_picker_reconcile_path_editor_return_owner(new_dialog, generation);
            }
            PickerOwnerApplyResult::Published(PickerOwnerAppliedPublication {
                previous,
                cancelled_close,
                lifecycle_generation: generation,
            })
        }
        PickerOwnerPublicationRequest::CompareSet {
            expected,
            new_dialog,
        } => apply_compare(expected, new_dialog),
        PickerOwnerPublicationRequest::CompareRemove {
            expected,
            pending,
            new_dialog,
        } => {
            if save_picker_pending_resubmit_transition() != Some(pending)
                || !save_picker_pending_resubmit_matches_native_removal(expected)
            {
                return PickerOwnerApplyResult::Stale {
                    actual: SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst),
                };
            }
            let result = apply_compare(expected.owner.dialog, new_dialog);
            if matches!(result, PickerOwnerApplyResult::Published(_)) {
                er_telemetry::counters::SAVE_PICKER_NATIVE_REMOVAL_EXACT_PUBLICATIONS
                    .fetch_add(1, Ordering::SeqCst);
                er_telemetry::counters::SAVE_PICKER_NATIVE_REMOVAL_LAST_OWNER_GENERATION
                    .store(expected.owner.generation, Ordering::SeqCst);
            }
            result
        }
    }
}

pub(crate) fn save_picker_path_editor_publish_owner(
    new_dialog: usize,
    job: usize,
) -> PickerOwnerPublicationDisposition {
    picker_owner_lifetime().publish_with(
        PickerOwnerPublicationRequest::Set { new_dialog, job },
        apply_picker_owner_publication_now,
    )
}

pub(crate) fn save_picker_path_editor_publish_owner_if_current(
    expected_dialog: usize,
    new_dialog: usize,
) -> PickerOwnerPublicationDisposition {
    picker_owner_lifetime().publish_with(
        PickerOwnerPublicationRequest::CompareSet {
            expected: expected_dialog,
            new_dialog,
        },
        apply_picker_owner_publication_now,
    )
}

fn path_editor_native_submit_identity_still_current_with(
    ticket: er_save_picker::PathEditorRequestTicket,
    token: PickerProfileRunToken,
    current_dialog: usize,
    token_lineage_is_current: impl FnOnce(PickerProfileRunToken) -> bool,
    read_vtable: impl FnMut(usize) -> Option<usize>,
) -> bool {
    ticket.dialog == token.dialog
        && picker_profile_token_still_current_with(
            token,
            current_dialog,
            token_lineage_is_current,
            read_vtable,
        )
}

fn path_editor_native_submit_identity_still_current(
    ticket: er_save_picker::PathEditorRequestTicket,
    token: PickerProfileRunToken,
) -> bool {
    path_editor_native_submit_identity_still_current_with(
        ticket,
        token,
        save_picker_live_profile_dialog(),
        |token| picker_owner_lifetime().token_lineage_is_current(token),
        |dialog| unsafe { safe_read_usize(dialog) },
    )
}

fn run_path_editor_native_phase_with<R>(
    ticket: er_save_picker::PathEditorRequestTicket,
    token: PickerProfileRunToken,
    current_dialog: usize,
    token_lineage_is_current: impl FnOnce(PickerProfileRunToken) -> bool,
    read_vtable: impl FnMut(usize) -> Option<usize>,
    phase: impl FnOnce() -> R,
) -> Option<R> {
    path_editor_native_submit_identity_still_current_with(
        ticket,
        token,
        current_dialog,
        token_lineage_is_current,
        read_vtable,
    )
    .then(phase)
}

fn run_path_editor_native_phase<R>(
    ticket: er_save_picker::PathEditorRequestTicket,
    token: PickerProfileRunToken,
    phase: impl FnOnce() -> R,
) -> Option<R> {
    if ticket.dialog != token.dialog {
        return None;
    }
    execute_picker_live_token_call_with(
        token,
        save_picker_live_profile_dialog,
        |dialog| unsafe { safe_read_usize(dialog) },
        |_| phase(),
    )
    .ok()
    .map(|(_, _, result)| result)
}

unsafe fn submit_path_editor(
    ticket: er_save_picker::PathEditorRequestTicket,
    token: PickerProfileRunToken,
) -> PathEditorSubmitDisposition {
    if !path_editor_native_submit_identity_still_current(ticket, token) {
        return PathEditorSubmitDisposition::Retryable;
    }
    let queue = ticket.dialog + SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET;
    dispatch_recipe_submit_with(
        software_keyboard_recipe(),
        |recipe| {
            if !install_software_keyboard_result_hooks() {
                append_autoload_debug(format_args!(
                    "save-picker-path: result lifecycle hooks unavailable; refusing a job whose intentionally-empty callback cannot be consumed safely"
                ));
                return false;
            }
            let queue_ready: unsafe extern "system" fn(usize) -> u8 =
                unsafe { std::mem::transmute(recipe.queue_ready) };
            run_path_editor_native_phase(ticket, token, || {
                er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_QUEUE_READY_CALLS
                    .fetch_add(1, Ordering::SeqCst);
                unsafe { queue_ready(queue) != 0 }
            })
            .unwrap_or(false)
        },
        |recipe| unsafe { submit_path_editor_with_recipe(ticket, token, recipe, queue) },
    )
}

fn activate_and_submit_path_editor_with(
    validate_before_activation: impl FnOnce() -> bool,
    activate: impl FnOnce() -> bool,
    abort: impl FnOnce(),
    submit_under_final_lease: impl FnOnce() -> bool,
) -> bool {
    if !validate_before_activation() || !activate() {
        return false;
    }
    // The last closure acquires the owner-lifetime lease, validates owner+vtable, and keeps the
    // lease until the native queue submit returns. Failure retires the unsubmitted active job.
    if !submit_under_final_lease() {
        abort();
        return false;
    }
    true
}

unsafe fn submit_path_editor_with_recipe(
    ticket: er_save_picker::PathEditorRequestTicket,
    token: PickerProfileRunToken,
    recipe: &SoftwareKeyboardRecipe,
    queue: usize,
) -> bool {
    if !path_editor_native_submit_identity_still_current(ticket, token) {
        return false;
    }
    let dialog = ticket.dialog;
    let initial = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return false;
        };
        let Some(path) = model.current_dir().to_str() else {
            return false;
        };
        path.encode_utf16()
            .chain(core::iter::once(0))
            .collect::<Vec<_>>()
    };

    let mut validator = [0_u64; SOFTWARE_KEYBOARD_VALIDATOR_SIZE / 8];
    let validator_ptr = validator.as_mut_ptr() as usize;
    let validator_init: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(recipe.validator_init) };
    let enter_name: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.enter_name) };
    let set_initial: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.set_initial) };
    let set_max: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(recipe.set_max) };
    let validator_dtor: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(recipe.validator_dtor) };
    unsafe {
        validator_init(validator_ptr);
        enter_name(validator_ptr, initial.as_ptr() as usize);
        set_max(validator_ptr, SOFTWARE_KEYBOARD_MAX_PATH_UNITS as i32);
        *((validator_ptr + SOFTWARE_KEYBOARD_VALIDATOR_MAX_6C_OFFSET) as *mut u32) =
            SOFTWARE_KEYBOARD_MAX_PATH_UNITS as u32;
        let flags = (validator_ptr + SOFTWARE_KEYBOARD_VALIDATOR_FLAGS_68_OFFSET) as *mut u32;
        *flags &= !2;
        set_initial(validator_ptr, initial.as_ptr() as usize);
    }
    debug_assert_eq!(
        unsafe { safe_read_i32(validator_ptr + SOFTWARE_KEYBOARD_VALIDATOR_MAX_60_OFFSET) },
        Some(SOFTWARE_KEYBOARD_MAX_PATH_UNITS as i32)
    );

    let Ok(base) = game_module_base() else {
        unsafe { validator_dtor(validator_ptr) };
        return false;
    };
    let allocator = match unsafe { safe_read_usize(base + GLOBAL_MENU_HEAP_ALLOCATOR_RVA) } {
        Some(allocator) if allocator != 0 && allocator != TITLE_OWNER_SCAN_START_ADDRESS => {
            allocator
        }
        _ => {
            unsafe { validator_dtor(validator_ptr) };
            return false;
        }
    };
    let heap_alloc: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(recipe.heap_alloc) };
    let memory = unsafe { heap_alloc(SOFTWARE_KEYBOARD_JOB_SIZE, 8, allocator) };
    if memory == 0 || memory == TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { validator_dtor(validator_ptr) };
        return false;
    }

    let config = SoftwareKeyboardConfig {
        max_units: SOFTWARE_KEYBOARD_MAX_PATH_UNITS as u32,
        mode: 1,
        padding: [0; 3],
        resource: TEXT_INPUT_RESOURCE.as_ptr(),
    };
    // This is an empty 64-byte MSVC std::function carrier. FUN_14081be30 consequently leaves
    // job+0x1a0 null. We do not fabricate that compiler-private ABI: submission is allowed only
    // after the exact FUN_14081d220 continuation hook is installed to consume our owned result.
    let intercepted_empty_callback = [0_usize; 8];
    let ctor: unsafe extern "system" fn(usize, usize, usize, usize, usize, u8, usize) -> usize =
        unsafe { std::mem::transmute(recipe.ctor) };
    let Some(job) = run_path_editor_native_phase(ticket, token, || unsafe {
        er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_CONSTRUCTOR_CALLS
            .fetch_add(1, Ordering::SeqCst);
        ctor(
            memory,
            dialog + SYSTEM_QUIT_DIALOG_MENU_WINDOW_LIST_50_OFFSET,
            validator_ptr,
            (&raw const config) as usize,
            initial.as_ptr() as usize,
            1,
            intercepted_empty_callback.as_ptr() as usize,
        )
    }) else {
        unsafe { validator_dtor(validator_ptr) };
        return false;
    };
    unsafe { validator_dtor(validator_ptr) };
    if job == 0 || job == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    if unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CALLBACK_1A0_OFFSET) } != Some(0) {
        SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-path: SoftwareKeyboardJob callback storage invariant failed job=0x{job:x}; refusing native submission"
        ));
        return false;
    }

    unsafe {
        let refcount = (job + MENU_JOB_REFCOUNT_08_OFFSET) as *mut std::sync::atomic::AtomicI32;
        (*refcount).fetch_add(1, Ordering::SeqCst);
    }
    let mut job_slot = job;
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.submit) };
    let submitted = activate_and_submit_path_editor_with(
        || path_editor_native_submit_identity_still_current(ticket, token),
        || match path_editor_lifecycle().activate(ticket, job) {
            Ok(()) => {
                SAVE_PICKER_PATH_EDITOR_WINDOW.reset(
                    er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
                );
                true
            }
            Err(reason) => {
                SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS
                    .fetch_add(1, Ordering::SeqCst);
                path_editor_named_rejection(match reason {
                    er_save_picker::PathEditorLifecycleRejection::InvalidJob => {
                        "submit-reused-or-invalid-job"
                    }
                    _ => "submit-ticket-no-longer-owned",
                });
                false
            }
        },
        || {
            let _ = path_editor_lifecycle().abort_active_submit(ticket, job);
            SAVE_PICKER_PATH_EDITOR_WINDOW.reset(
                er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
            );
            path_editor_named_rejection("submit-token-changed-after-activation");
        },
        || {
            run_path_editor_native_phase(ticket, token, || {
                er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_NATIVE_SUBMIT_CALLS
                    .fetch_add(1, Ordering::SeqCst);
                unsafe { submit(queue, (&raw mut job_slot) as usize) };
            })
            .is_some()
        },
    );
    if !submitted {
        return false;
    }
    append_autoload_debug(format_args!(
        "save-picker-path: submitted native SoftwareKeyboardJob=0x{job:x} dialog=0x{dialog:x} generation={} queue=0x{queue:x} initial_units={} max_units={SOFTWARE_KEYBOARD_MAX_PATH_UNITS}",
        ticket.generation,
        initial.len().saturating_sub(1)
    ));
    true
}

/// Apply the editor result and report whether picker-visible model state changed. Cancellation is
/// deliberately a no-op: rebuilding unchanged rows after the 02_990 proxy stopped invalidated the
/// embedded ProfileLoad ScrollBarV proxy, then the next scrollbar pump dispatched through null.
fn apply_path_editor_outcome_to_model(
    model: &mut er_save_picker::SavePickerModel,
    outcome: PathEditorOutcome,
) -> (bool, Option<PathEditorLifecycleStatus>) {
    let before_dir = model.current_dir().to_path_buf();
    let before_status = model.status_message().cloned();
    let lifecycle_status = match outcome {
        PathEditorOutcome::Accepted(path) => match model.set_current_dir_from_text(&path) {
            Ok(changed) => {
                SAVE_PICKER_PATH_EDITOR_APPLIED_DIRECTORIES.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-picker-path: committed accepted directory changed={changed} exact='{}'",
                    model.current_dir().display()
                ));
                Some(PATH_EDITOR_STATUS_APPLIED_DIRECTORY)
            }
            Err(reason) => {
                SAVE_PICKER_PATH_EDITOR_VALIDATION_REJECTIONS.fetch_add(1, Ordering::SeqCst);
                model.set_status_message(reason.status_message());
                // Keep what the user typed on screen, marked invalid, so it can be corrected. Note
                // the ORDER: `set_status_message` does not clear, but `set_current_dir_from_text`
                // clears on success -- so a later valid entry drops this automatically.
                model.set_rejected_path_text(&path);
                append_autoload_debug(format_args!(
                    "save-picker-path: rejected accepted text reason={reason:?}; keeping '{path}' on the control as invalid; directory remains '{}'",
                    model.current_dir().display()
                ));
                Some(PATH_EDITOR_STATUS_VALIDATION_REJECTED)
            }
        },
        PathEditorOutcome::Cancelled => {
            append_autoload_debug(format_args!(
                "save-picker-path: cancel consumed without row rebuild; directory remains '{}'",
                model.current_dir().display()
            ));
            None
        }
        PathEditorOutcome::TextUnreadable => {
            SAVE_PICKER_PATH_EDITOR_VALIDATION_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            model.set_status_message(er_save_picker::PickerStatusMessage::new(
                "PATH TEXT UNREADABLE",
                "The native editor returned invalid UTF-16; the folder was not changed.",
            ));
            Some(PATH_EDITOR_STATUS_VALIDATION_REJECTED)
        }
    };
    (
        before_dir != model.current_dir() || before_status.as_ref() != model.status_message(),
        lifecycle_status,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathEditorOutcomeTransition {
    ReturnReopenArmed,
    ReturnReopenArmedWithRefresh,
    Rejected,
}

/// Route one exact-generation terminal editor result through the production transition. Every
/// terminal outcome arms the no-native-close parent return. A changed directory/status additionally
/// queues a content generation, but that request must wait behind the matching return latch until
/// native owner disappearance; it never closes the parent that SoftwareKeyboard just finished.
fn apply_path_editor_outcome_to_transition_with(
    model: &mut er_save_picker::SavePickerModel,
    ticket: er_save_picker::PathEditorRequestTicket,
    current_dialog: usize,
    generation_owned: bool,
    outcome: PathEditorOutcome,
    mut request_refresh: impl FnMut() -> bool,
    mut arm_return_reopen: impl FnMut() -> bool,
    mut record_status: impl FnMut(PathEditorLifecycleStatus),
) -> PathEditorOutcomeTransition {
    // Reject a delayed/new-owner/ABA result before it can mutate the current generation's model.
    if !generation_owned || (current_dialog != 0 && current_dialog != ticket.dialog) {
        return PathEditorOutcomeTransition::Rejected;
    }
    let (changed, lifecycle_status) = apply_path_editor_outcome_to_model(model, outcome);
    if let Some(status) = lifecycle_status {
        record_status(status);
    }
    if !arm_return_reopen() {
        return PathEditorOutcomeTransition::Rejected;
    }
    if changed && current_dialog == ticket.dialog && request_refresh() {
        record_status(PATH_EDITOR_STATUS_REBUILD_SCHEDULED);
        PathEditorOutcomeTransition::ReturnReopenArmedWithRefresh
    } else {
        // Owner-zero already has the only transition needed: fresh staging reads the retained,
        // already-mutated model. A refresh scheduling failure must not wedge that return.
        PathEditorOutcomeTransition::ReturnReopenArmed
    }
}

fn schedule_path_editor_refresh(dialog: usize) -> bool {
    let scheduled = save_picker_schedule_refresh_request(dialog, "path-editor-visible-change");
    if scheduled {
        // Compatibility oracle name retained for historical runtime comparisons; the action is now
        // a fresh-owner refresh, never an in-place list rebuild.
        SAVE_PICKER_PATH_EDITOR_REBUILDS_SCHEDULED.fetch_add(1, Ordering::SeqCst);
    }
    scheduled
}

fn apply_path_editor_outcome_transaction_owned(
    ticket: er_save_picker::PathEditorRequestTicket,
    outcome: PathEditorOutcome,
    transaction_identity: er_save_picker::PathEditorPickerIdentity,
) -> Option<PathEditorLifecycleStatus> {
    SAVE_PICKER_PATH_EDITOR_WINDOW
        .reset(er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst));
    let current_dialog = transaction_identity.current_dialog;
    let mut guard = crate::experiments::save_picker::active_save_picker_lock();
    let Some(model) = guard.as_mut() else {
        SAVE_PICKER_PATH_EDITOR_VALIDATION_REJECTIONS.fetch_add(1, Ordering::SeqCst);
        let _ = save_picker_arm_path_editor_return_reopen_transaction_owned(
            ticket,
            transaction_identity,
            "path-editor-result-no-model",
        );
        return Some(PATH_EDITOR_STATUS_VALIDATION_REJECTED);
    };
    let lifecycle_status = std::cell::Cell::new(None);
    let _ = apply_path_editor_outcome_to_transition_with(
        model,
        ticket,
        current_dialog,
        true,
        outcome,
        || schedule_path_editor_refresh(ticket.dialog),
        || {
            save_picker_arm_path_editor_return_reopen_transaction_owned(
                ticket,
                transaction_identity,
                "path-editor-result",
            )
        },
        |status| lifecycle_status.set(Some(status)),
    );
    lifecycle_status.get()
}

/// Pointer-free result/reconcile half of the path-editor pump. Generic MenuWindow posts may run
/// this half because it never reads a ProfileLoad object, queue, constructor, or submit function.
pub(crate) unsafe fn save_picker_menu_pump_path_editor_pointer_free() {
    expire_stopped_path_editor_window();
    let identity = current_picker_identity();
    // Consume an exact completed generation before reconciliation can invalidate it. Owner-zero is
    // allowed only for this completed-result handoff: it means the parent finished natively and the
    // result becomes a no-close reopen, never a stale dialog call.
    let outcome = match path_editor_lifecycle().take_completed_for_owner_transition(identity) {
        Ok(outcome) => outcome,
        Err(_) => {
            SAVE_PICKER_PATH_EDITOR_STALE_RESULTS.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_PATH_EDITOR_LIFECYCLE_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            None
        }
    };
    if let Some((ticket, outcome)) = outcome {
        match path_editor_lifecycle().with_terminal_result_transaction(identity, ticket, || {
            apply_path_editor_outcome_transaction_owned(ticket, outcome, identity)
        }) {
            Ok(transaction) => {
                if let Some(status) = transaction.result {
                    path_editor_set_status(status);
                }
                if transaction.reconcile.cancelled_close.is_some() {
                    SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_CANCELS.fetch_add(1, Ordering::SeqCst);
                }
            }
            Err(_) => {
                SAVE_PICKER_PATH_EDITOR_STALE_RESULTS.fetch_add(1, Ordering::SeqCst);
                path_editor_named_rejection("terminal-result-owner-generation-changed");
            }
        }
        return;
    }
    let reconciled = path_editor_lifecycle().reconcile_identity(identity);
    if reconciled.cancelled_close.is_some() {
        SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_CANCELS.fetch_add(1, Ordering::SeqCst);
    }
    if reconciled.invalidated {
        SAVE_PICKER_PATH_EDITOR_WINDOW
            .reset(er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst));
        path_editor_named_rejection("picker-mode-or-dialog-identity-lost");
    }
}

/// Native submit half. The caller supplies the exact current 05_010 Run token; this function and
/// every native queue/build/submit boundary independently revalidate its dialog and vtable.
pub(crate) unsafe fn save_picker_menu_pump_path_editor_native_submit(token: PickerProfileRunToken) {
    if !save_picker_profile_token_still_current(token) {
        path_editor_named_rejection("native-submit-profile-token-not-current");
        return;
    }
    let identity = current_picker_identity();
    if pump_path_editor_submit_with(
        path_editor_lifecycle(),
        identity,
        |ticket, _| unsafe { submit_path_editor(ticket, token) },
        record_path_editor_submit_event,
    )
    .is_err()
    {
        path_editor_named_rejection("menu-pump-mode-dialog-generation-mismatch");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_keyboard_config_matches_the_static_constructor_copy() {
        assert_eq!(core::mem::size_of::<SoftwareKeyboardConfig>(), 0x10);
        assert_eq!(SOFTWARE_KEYBOARD_VALIDATOR_SIZE, 0x70);
        assert_eq!(SOFTWARE_KEYBOARD_JOB_SIZE, 0x1a8);
        assert_eq!(
            String::from_utf16(&TEXT_INPUT_RESOURCE[..TEXT_INPUT_RESOURCE.len() - 1]).unwrap(),
            "02_990_TextInput"
        );
    }

    #[test]
    fn terminal_editor_window_releases_active_job_before_proxy_teardown_returns() {
        use std::cell::Cell;

        let tracker = PathEditorWindowTracker::new();
        let active = er_save_picker::PathEditorActiveProvenance {
            generation: 7,
            job: 0x282f_2fc0,
        };
        assert_eq!(
            tracker.note_state(0x8519_2080, 0, 41, Some(active), |_| None),
            PathEditorWindowDisposition::Live {
                first_observation: true
            }
        );

        let released = Cell::new(false);
        let disposition = tracker.note_state(
            0x8519_2080,
            MENU_JOB_STATE_FAILED,
            41,
            Some(active),
            |expected| {
                assert_eq!(expected, active);
                // Production clears the native-window association before publishing cancellation;
                // the caller cannot continue into later proxy work until this callback returns.
                assert_eq!(tracker.window.load(Ordering::SeqCst), 0);
                released.set(true);
                Some(expected.job)
            },
        );
        assert!(released.get());
        assert_eq!(
            disposition,
            PathEditorWindowDisposition::TerminalCancelled { job: active.job }
        );
    }

    #[test]
    fn reset_reopen_same_address_late_terminal_cannot_cancel_new_generation() {
        let tracker = PathEditorWindowTracker::new();
        let coordinator = er_save_picker::PathEditorCoordinator::<
            &'static str,
            er_save_picker::NoopPathEditorLifecyclePublisher,
        >::new(er_save_picker::NoopPathEditorLifecyclePublisher);
        let identity = er_save_picker::PathEditorPickerIdentity {
            picker_mode_active: true,
            current_dialog: 0x8519_0000,
        };
        let reused_job = 0x282f_2fc0;
        let old_window = 0x8519_2080;
        let new_window = 0x8519_3080;

        coordinator
            .request(identity, identity.current_dialog)
            .unwrap();
        coordinator
            .with_submit(identity, |ticket, coordinator| {
                coordinator.activate(ticket, reused_job).unwrap();
            })
            .unwrap();
        let generation_a = coordinator.active_provenance().unwrap();
        assert!(matches!(
            tracker.note_state(
                old_window,
                MENU_JOB_STATE_CONTINUE,
                41,
                Some(generation_a),
                |_| None
            ),
            PathEditorWindowDisposition::Live { .. }
        ));
        assert_eq!(
            coordinator.record_active_result(
                generation_a,
                "generation-a-complete",
                PathEditorLifecycleStatus::NativeAccept
            ),
            Some(er_save_picker::PathEditorResultOwnership::Current)
        );
        assert_eq!(
            coordinator.take_completed(identity),
            Ok(Some("generation-a-complete"))
        );
        match coordinator.begin_reset(identity) {
            er_save_picker::PathEditorResetStart::Acquired { guard, .. } => drop(guard),
            _ => panic!("reset must be acquired after generation A completes"),
        }
        tracker.reset(42);

        coordinator
            .request(identity, identity.current_dialog)
            .unwrap();
        coordinator
            .with_submit(identity, |ticket, coordinator| {
                coordinator.activate(ticket, reused_job).unwrap();
            })
            .unwrap();
        let generation_b = coordinator.active_provenance().unwrap();
        assert_ne!(generation_b.generation, generation_a.generation);
        assert_eq!(generation_b.job, generation_a.job);

        assert_eq!(
            tracker.note_state(
                old_window,
                MENU_JOB_STATE_FAILED,
                42,
                Some(generation_b),
                |_| panic!("an unbound late terminal event must fail closed")
            ),
            PathEditorWindowDisposition::Unowned
        );
        assert_eq!(coordinator.active_provenance(), Some(generation_b));

        assert!(matches!(
            tracker.note_state(
                new_window,
                MENU_JOB_STATE_CONTINUE,
                43,
                Some(generation_b),
                |_| None
            ),
            PathEditorWindowDisposition::Live { .. }
        ));
        assert_eq!(
            tracker.note_state(
                new_window,
                MENU_JOB_STATE_FAILED,
                43,
                Some(generation_b),
                |expected| {
                    (coordinator.record_active_result(
                        expected,
                        "generation-b-cancelled",
                        PathEditorLifecycleStatus::NativeCancel,
                    ) == Some(er_save_picker::PathEditorResultOwnership::Current))
                    .then_some(expected.job)
                }
            ),
            PathEditorWindowDisposition::TerminalCancelled { job: reused_job }
        );
        assert_eq!(coordinator.active_provenance(), None);
        assert_eq!(
            coordinator.take_completed(identity),
            Ok(Some("generation-b-cancelled"))
        );
    }

    #[test]
    fn stopped_editor_window_watchdog_releases_stale_active_job_once() {
        use std::cell::Cell;

        let tracker = PathEditorWindowTracker::new();
        let active = er_save_picker::PathEditorActiveProvenance {
            generation: 11,
            job: 0x282f_2fc0,
        };
        let window = 0x8519_2080;
        assert!(matches!(
            tracker.note_state(window, MENU_JOB_STATE_CONTINUE, 100, Some(active), |_| None),
            PathEditorWindowDisposition::Live { .. }
        ));
        let releases = Cell::new(0);
        assert_eq!(
            tracker.expire_stopped(102, Some(active), |_| {
                releases.set(releases.get() + 1);
                Some(active.job)
            }),
            PathEditorWindowDisposition::Unowned
        );
        assert_eq!(releases.get(), 0);
        assert_eq!(
            tracker.expire_stopped(103, Some(active), |expected| {
                assert_eq!(expected, active);
                assert_eq!(tracker.window.load(Ordering::SeqCst), 0);
                releases.set(releases.get() + 1);
                Some(expected.job)
            }),
            PathEditorWindowDisposition::StoppedCancelled {
                job: active.job,
                window,
                stale_ticks: PATH_EDITOR_WINDOW_STALE_PROFILE_TICKS,
            }
        );
        assert_eq!(releases.get(), 1);
        assert_eq!(
            tracker.expire_stopped(104, Some(active), |_| {
                releases.set(releases.get() + 1);
                Some(active.job)
            }),
            PathEditorWindowDisposition::Unowned
        );
        assert_eq!(releases.get(), 1);
    }

    fn test_ticket() -> er_save_picker::PathEditorRequestTicket {
        er_save_picker::PathEditorRequestTicket {
            dialog: 0x5000,
            generation: 7,
        }
    }

    fn drive_outcome_to_transition(
        model: &mut er_save_picker::SavePickerModel,
        outcome: PathEditorOutcome,
        current_dialog: usize,
        request_result: bool,
        reopen_result: bool,
    ) -> (PathEditorOutcomeTransition, Vec<&'static str>) {
        let events = std::cell::RefCell::new(Vec::new());
        let transition = apply_path_editor_outcome_to_transition_with(
            model,
            test_ticket(),
            current_dialog,
            true,
            outcome,
            || {
                events.borrow_mut().push("request");
                request_result
            },
            || {
                events.borrow_mut().push("reopen");
                reopen_result
            },
            |_| {},
        );
        (transition, events.into_inner())
    }

    #[test]
    fn cancelled_outcome_arms_one_no_close_reopen_and_no_refresh() {
        let root = std::env::temp_dir().join(format!(
            "er-effects-path-editor-cancel-no-refresh-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut model = er_save_picker::SavePickerModel::open(&root, "sl2");
        model.set_status_message(er_save_picker::PickerStatusMessage::new(
            "KEEP",
            "Cancellation must preserve this visible state.",
        ));
        let before_dir = model.current_dir().to_path_buf();
        let before_status = model.status_message().cloned();

        let (transition, events) = drive_outcome_to_transition(
            &mut model,
            PathEditorOutcome::Cancelled,
            test_ticket().dialog,
            true,
            true,
        );
        assert_eq!(transition, PathEditorOutcomeTransition::ReturnReopenArmed);
        assert_eq!(events, ["reopen"]);
        assert_eq!(model.current_dir(), before_dir);
        assert_eq!(model.status_message(), before_status.as_ref());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_current_terminal_outcome_arms_one_return_and_only_changed_content_refreshes() {
        let root = std::env::temp_dir().join(format!(
            "er-effects-path-editor-terminal-return-{}",
            std::process::id()
        ));
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();

        let mut unreadable = er_save_picker::SavePickerModel::open(&root, "sl2");
        let (transition, events) = drive_outcome_to_transition(
            &mut unreadable,
            PathEditorOutcome::TextUnreadable,
            test_ticket().dialog,
            true,
            true,
        );
        assert_eq!(
            transition,
            PathEditorOutcomeTransition::ReturnReopenArmedWithRefresh
        );
        assert_eq!(events, ["reopen", "request"]);
        assert_eq!(
            unreadable.status_message().map(|status| status.headline()),
            Some("PATH TEXT UNREADABLE")
        );

        let mut validation_error = er_save_picker::SavePickerModel::open(&root, "sl2");
        let (transition, events) = drive_outcome_to_transition(
            &mut validation_error,
            PathEditorOutcome::Accepted(root.join("missing").to_string_lossy().into_owned()),
            test_ticket().dialog,
            true,
            true,
        );
        assert_eq!(
            transition,
            PathEditorOutcomeTransition::ReturnReopenArmedWithRefresh
        );
        assert_eq!(events, ["reopen", "request"]);
        assert!(validation_error.status_message().is_some());

        let mut unchanged = er_save_picker::SavePickerModel::open(&root, "sl2");
        let root_text = root.to_string_lossy().into_owned();
        let (transition, events) = drive_outcome_to_transition(
            &mut unchanged,
            PathEditorOutcome::Accepted(root_text),
            test_ticket().dialog,
            true,
            true,
        );
        assert_eq!(transition, PathEditorOutcomeTransition::ReturnReopenArmed);
        assert_eq!(events, ["reopen"]);

        let mut changed = er_save_picker::SavePickerModel::open(&root, "sl2");
        let (transition, events) = drive_outcome_to_transition(
            &mut changed,
            PathEditorOutcome::Accepted(child.to_string_lossy().into_owned()),
            test_ticket().dialog,
            true,
            true,
        );
        assert_eq!(
            transition,
            PathEditorOutcomeTransition::ReturnReopenArmedWithRefresh
        );
        assert_eq!(events, ["reopen", "request"]);
        assert_eq!(changed.current_dir(), child.as_path());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_owner_zero_terminal_outcome_arms_one_return_and_retains_visible_change() {
        let root = std::env::temp_dir().join(format!(
            "er-effects-path-editor-owner-zero-{}",
            std::process::id()
        ));
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();

        let outcomes = [
            PathEditorOutcome::Cancelled,
            PathEditorOutcome::TextUnreadable,
            PathEditorOutcome::Accepted(root.to_string_lossy().into_owned()),
            PathEditorOutcome::Accepted(root.join("missing").to_string_lossy().into_owned()),
            PathEditorOutcome::Accepted(child.to_string_lossy().into_owned()),
        ];
        for outcome in outcomes {
            let mut model = er_save_picker::SavePickerModel::open(&root, "sl2");
            let (transition, events) =
                drive_outcome_to_transition(&mut model, outcome, 0, true, true);
            assert_eq!(transition, PathEditorOutcomeTransition::ReturnReopenArmed);
            assert_eq!(events, ["reopen"]);
        }

        let mut changed = er_save_picker::SavePickerModel::open(&root, "sl2");
        let _ = drive_outcome_to_transition(
            &mut changed,
            PathEditorOutcome::Accepted(child.to_string_lossy().into_owned()),
            0,
            true,
            true,
        );
        assert_eq!(changed.current_dir(), child.as_path());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejected_content_refresh_keeps_the_already_armed_no_close_return() {
        let root = std::env::temp_dir().join(format!(
            "er-effects-path-editor-refresh-failure-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut model = er_save_picker::SavePickerModel::open(&root, "sl2");

        let (transition, events) = drive_outcome_to_transition(
            &mut model,
            PathEditorOutcome::TextUnreadable,
            test_ticket().dialog,
            false,
            true,
        );
        assert_eq!(transition, PathEditorOutcomeTransition::ReturnReopenArmed);
        assert_eq!(events, ["reopen", "request"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_new_owner_and_same_address_new_generation_cannot_mutate_or_reopen() {
        let root = std::env::temp_dir().join(format!(
            "er-effects-path-editor-stale-generation-{}",
            std::process::id()
        ));
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        for (current_dialog, generation_owned) in
            [(0x6000, true), (test_ticket().dialog, false), (0, false)]
        {
            let mut model = er_save_picker::SavePickerModel::open(&root, "sl2");
            let before = model.current_dir().to_path_buf();
            let events = std::cell::RefCell::new(Vec::new());
            assert_eq!(
                apply_path_editor_outcome_to_transition_with(
                    &mut model,
                    test_ticket(),
                    current_dialog,
                    generation_owned,
                    PathEditorOutcome::Accepted(child.to_string_lossy().into_owned()),
                    || {
                        events.borrow_mut().push("request");
                        true
                    },
                    || {
                        events.borrow_mut().push("reopen");
                        true
                    },
                    |_| {},
                ),
                PathEditorOutcomeTransition::Rejected
            );
            assert_eq!(model.current_dir(), before.as_path());
            assert!(events.into_inner().is_empty());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_submit_phases_require_exact_token_and_revalidate_last_moment() {
        let ticket = test_ticket();
        let token = PickerProfileRunToken {
            job: 0x4100,
            list: 0x4200,
            dialog: ticket.dialog,
            owner_generation: 1,
            job_lineage: 1,
            run_lineage: 1,
            observed_vtable: 0x142b229f8,
            expected_vtable: 0x142b229f8,
        };
        let queue_ready = std::cell::Cell::new(0);
        let constructor = std::cell::Cell::new(0);
        let submit = std::cell::Cell::new(0);
        assert_eq!(
            run_path_editor_native_phase_with(
                ticket,
                token,
                ticket.dialog,
                |_| true,
                |_| Some(token.expected_vtable),
                || queue_ready.set(queue_ready.get() + 1),
            ),
            Some(())
        );
        assert_eq!(
            run_path_editor_native_phase_with(
                ticket,
                token,
                ticket.dialog,
                |_| true,
                |_| Some(token.expected_vtable),
                || constructor.set(constructor.get() + 1),
            ),
            Some(())
        );
        assert_eq!(
            run_path_editor_native_phase_with(
                ticket,
                token,
                ticket.dialog,
                |_| true,
                |_| Some(token.expected_vtable),
                || submit.set(submit.get() + 1),
            ),
            Some(())
        );
        assert_eq!(
            (queue_ready.get(), constructor.get(), submit.get()),
            (1, 1, 1)
        );

        let interphase_owner = std::cell::Cell::new(ticket.dialog);
        assert_eq!(
            run_path_editor_native_phase_with(
                ticket,
                token,
                interphase_owner.get(),
                |_| true,
                |_| Some(token.expected_vtable),
                || {
                    queue_ready.set(queue_ready.get() + 1);
                    interphase_owner.set(0x6000);
                },
            ),
            Some(())
        );
        assert_eq!(
            run_path_editor_native_phase_with(
                ticket,
                token,
                interphase_owner.get(),
                |_| true,
                |_| panic!("inter-phase owner loss rejects before constructor vtable read"),
                || constructor.set(constructor.get() + 1),
            ),
            None
        );
        assert_eq!(queue_ready.get(), 2);
        assert_eq!(constructor.get(), 1);

        assert_eq!(
            run_path_editor_native_phase_with(
                ticket,
                token,
                ticket.dialog,
                |_| true,
                |_| Some(0x142b_22000),
                || submit.set(submit.get() + 1),
            ),
            None
        );
        assert_eq!(submit.get(), 1, "vtable mismatch must not invoke submit");
        assert_eq!(
            run_path_editor_native_phase_with(
                ticket,
                token,
                0x6000,
                |_| true,
                |_| panic!("owner mismatch rejects before vtable read"),
                || queue_ready.set(queue_ready.get() + 1),
            ),
            None
        );
        assert_eq!(queue_ready.get(), 2);
    }

    #[test]
    fn production_activation_wrapper_rechecks_after_activation_before_final_submit() {
        let published_owner = std::cell::Cell::<usize>::new(0x5000);
        let observed_vtable = std::cell::Cell::<usize>::new(0x142b229f8);
        let activated = std::cell::Cell::new(0);
        let aborted = std::cell::Cell::new(0);
        let submits = std::cell::Cell::new(0);
        let validate = || published_owner.get() == 0x5000 && observed_vtable.get() == 0x142b229f8;
        assert!(!activate_and_submit_path_editor_with(
            validate,
            || {
                activated.set(activated.get() + 1);
                published_owner.set(0x6000);
                true
            },
            || aborted.set(aborted.get() + 1),
            || {
                if !validate() {
                    return false;
                }
                submits.set(submits.get() + 1);
                true
            },
        ));
        assert_eq!(activated.get(), 1);
        assert_eq!(aborted.get(), 1);
        assert_eq!(submits.get(), 0);

        published_owner.set(0x5000);
        observed_vtable.set(0x142b229f8);
        assert!(!activate_and_submit_path_editor_with(
            validate,
            || {
                activated.set(activated.get() + 1);
                observed_vtable.set(0x142b22000);
                true
            },
            || aborted.set(aborted.get() + 1),
            || {
                if !validate() {
                    return false;
                }
                submits.set(submits.get() + 1);
                true
            },
        ));
        assert_eq!(activated.get(), 2);
        assert_eq!(aborted.get(), 2);
        assert_eq!(submits.get(), 0);

        observed_vtable.set(0x142b229f8);
        assert!(activate_and_submit_path_editor_with(
            validate,
            || {
                activated.set(activated.get() + 1);
                true
            },
            || aborted.set(aborted.get() + 1),
            || {
                if !validate() {
                    return false;
                }
                submits.set(submits.get() + 1);
                true
            },
        ));
        assert_eq!(activated.get(), 3);
        assert_eq!(aborted.get(), 2);
        assert_eq!(submits.get(), 1);
    }

    #[test]
    fn path_limit_exceeds_the_native_name_presets_without_becoming_unbounded() {
        assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS > 16);
        assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS <= 1024);
    }

    #[test]
    fn accepted_continuation_signature_and_null_callback_boundary_match_1162() {
        use iced_x86::{Decoder, DecoderOptions, Mnemonic, Register};

        assert_eq!(SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_RVA, 0x81d220);
        assert_eq!(SOFTWARE_KEYBOARD_JOB_CALLBACK_1A0_OFFSET, 0x1a0);
        assert_eq!(
            SOFTWARE_KEYBOARD_JOB_CALLBACK_1A0_OFFSET + core::mem::size_of::<usize>(),
            SOFTWARE_KEYBOARD_JOB_SIZE
        );
        assert_eq!(
            SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_SIG,
            &[
                0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8d,
                0x6c, 0x24, 0xd9,
            ]
        );
        let mut decoder = Decoder::with_ip(
            64,
            SOFTWARE_KEYBOARD_ACCEPT_CONTINUATION_SIG,
            0x14081d220,
            DecoderOptions::NONE,
        );
        let instructions = (0..8).map(|_| decoder.decode()).collect::<Vec<_>>();
        assert_eq!(
            instructions
                .iter()
                .map(|instruction| instruction.mnemonic())
                .collect::<Vec<_>>(),
            [
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Lea,
            ]
        );
        assert_eq!(instructions[0].op0_register(), Register::RBP);
        assert_eq!(instructions[7].op0_register(), Register::RBP);
        assert_eq!(instructions[7].memory_base(), Register::RSP);
        assert_eq!(instructions[7].memory_displacement64(), u64::MAX - 0x26);
        assert!(!decoder.can_decode());
    }

    #[test]
    fn callback_provenance_gates_same_address_reuse_and_evicted_null_jobs() {
        let mut job = [0_usize; SOFTWARE_KEYBOARD_JOB_SIZE / core::mem::size_of::<usize>()];
        let job_ptr = job.as_mut_ptr() as usize;
        assert_eq!(
            unsafe { software_keyboard_job_callback_provenance(job_ptr, true) },
            (Some(0), SoftwareKeyboardCallbackProvenance::OwnedNull)
        );
        job[SOFTWARE_KEYBOARD_JOB_CALLBACK_1A0_OFFSET / core::mem::size_of::<usize>()] = 0x1234;
        assert_eq!(
            unsafe { software_keyboard_job_callback_provenance(job_ptr, true) },
            (
                Some(0x1234),
                SoftwareKeyboardCallbackProvenance::ForeignNonNull
            )
        );
        assert_eq!(
            software_keyboard_callback_provenance(false, Some(0)),
            SoftwareKeyboardCallbackProvenance::OrphanedNull
        );
        assert_eq!(
            software_keyboard_callback_provenance(true, None),
            SoftwareKeyboardCallbackProvenance::UnreadableOwned
        );
        assert_eq!(
            software_keyboard_callback_provenance(false, None),
            SoftwareKeyboardCallbackProvenance::UnreadableForeign
        );
    }

    #[test]
    fn result_gate_suppresses_owned_accept_and_forwards_cancel_and_foreign_once() {
        use std::cell::Cell;

        let accepts = Cell::new(0);
        let forwards = Cell::new(0);
        let cancels = Cell::new(0);
        assert_eq!(
            dispatch_software_keyboard_result_gate_with(
                SoftwareKeyboardCallbackProvenance::OwnedNull,
                MENU_JOB_STATE_SUCCESS,
                || {
                    accepts.set(accepts.get() + 1);
                    0xa1
                },
                || {
                    forwards.set(forwards.get() + 1);
                    (0xff, MENU_JOB_STATE_FAILED)
                },
                || cancels.set(cancels.get() + 1),
            ),
            0xa1
        );
        assert_eq!((accepts.get(), forwards.get(), cancels.get()), (1, 0, 0));

        assert_eq!(
            dispatch_software_keyboard_result_gate_with(
                SoftwareKeyboardCallbackProvenance::OwnedNull,
                0,
                || panic!("cancel must not enter the acceptance path"),
                || {
                    forwards.set(forwards.get() + 1);
                    (0xb2, MENU_JOB_STATE_FAILED)
                },
                || cancels.set(cancels.get() + 1),
            ),
            0xb2
        );
        assert_eq!((accepts.get(), forwards.get(), cancels.get()), (1, 1, 1));

        // A non-null callback wins over a matching retired address: this is allocator reuse by a
        // valid foreign job, so both acceptance routes preserve the original exactly once.
        assert_eq!(
            dispatch_software_keyboard_result_gate_with(
                software_keyboard_callback_provenance(true, Some(0x1234)),
                MENU_JOB_STATE_SUCCESS,
                || panic!("same-address foreign acceptance must not be consumed"),
                || {
                    forwards.set(forwards.get() + 1);
                    (0xc3, MENU_JOB_STATE_SUCCESS)
                },
                || panic!("same-address foreign result must not become our cancellation"),
            ),
            0xc3
        );
        assert_eq!((accepts.get(), forwards.get(), cancels.get()), (1, 2, 1));
    }

    #[test]
    fn production_completion_writes_exact_native_result_and_rejects_invalid_storage() {
        let mut result = [0xcccccccc_u32, 0xdddddddd_u32];
        let mut time = 0_usize;
        let base = game_module_base().unwrap();
        assert!(software_keyboard_writable_range(
            result.as_mut_ptr() as usize,
            8
        ));
        assert!(software_keyboard_writable_range(
            (&raw mut time) as usize,
            core::mem::size_of::<usize>()
        ));
        assert!(unsafe {
            complete_software_keyboard_job_success(
                result.as_mut_ptr() as usize,
                (&raw mut time) as usize,
            )
        });
        assert_eq!(result, [MENU_JOB_STATE_SUCCESS as u32, 0]);
        assert_eq!(time, base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA);

        let untouched = result;
        assert!(!unsafe { complete_software_keyboard_job_success(0, (&raw mut time) as usize) });
        assert_eq!(result, untouched);
    }

    #[test]
    fn accepted_success_completes_before_current_or_stale_publication() {
        use std::cell::Cell;

        const DIALOG: usize = 0x5000;
        const JOB: usize = 0x7000;
        let coordinator = er_save_picker::PathEditorCoordinator::<
            &'static str,
            er_save_picker::NoopPathEditorLifecyclePublisher,
        >::new(er_save_picker::NoopPathEditorLifecyclePublisher);
        coordinator.request(test_identity(DIALOG), DIALOG).unwrap();
        coordinator
            .with_submit(test_identity(DIALOG), |ticket, coordinator| {
                coordinator.activate(ticket, JOB)
            })
            .unwrap()
            .unwrap()
            .unwrap();

        let mut current_result = [0_u32; 2];
        let mut current_time = 0_usize;
        let disposition = consume_software_keyboard_success_with(
            SoftwareKeyboardCallbackProvenance::OwnedNull,
            || unsafe {
                complete_software_keyboard_job_success(
                    current_result.as_mut_ptr() as usize,
                    (&raw mut current_time) as usize,
                )
            },
            || coordinator.record_result(JOB, "current", PATH_EDITOR_STATUS_NATIVE_ACCEPT),
        );
        assert_eq!(disposition, SoftwareKeyboardSuccessDisposition::Current);
        assert_eq!(current_result, [MENU_JOB_STATE_SUCCESS as u32, 0]);
        assert_eq!(
            coordinator.take_completed(test_identity(DIALOG)),
            Ok(Some("current"))
        );

        let stale_records = Cell::new(0);
        let mut stale_result = [0_u32; 2];
        let mut stale_time = 0_usize;
        let disposition = consume_software_keyboard_success_with(
            SoftwareKeyboardCallbackProvenance::OwnedNull,
            || unsafe {
                complete_software_keyboard_job_success(
                    stale_result.as_mut_ptr() as usize,
                    (&raw mut stale_time) as usize,
                )
            },
            || {
                stale_records.set(stale_records.get() + 1);
                coordinator.record_result(JOB, "stale", PATH_EDITOR_STATUS_NATIVE_ACCEPT)
            },
        );
        assert_eq!(disposition, SoftwareKeyboardSuccessDisposition::Stale);
        assert_eq!(stale_records.get(), 1);
        assert_eq!(stale_result, [MENU_JOB_STATE_SUCCESS as u32, 0]);
        assert_eq!(coordinator.take_completed(test_identity(DIALOG)), Ok(None));
    }

    #[test]
    fn invalid_completion_and_unreadable_owned_callback_never_publish() {
        use std::cell::Cell;

        const DIALOG: usize = 0x5000;
        const JOB: usize = 0x7000;
        let coordinator = er_save_picker::PathEditorCoordinator::<
            &'static str,
            er_save_picker::NoopPathEditorLifecyclePublisher,
        >::new(er_save_picker::NoopPathEditorLifecyclePublisher);
        coordinator.request(test_identity(DIALOG), DIALOG).unwrap();
        coordinator
            .with_submit(test_identity(DIALOG), |ticket, coordinator| {
                coordinator.activate(ticket, JOB)
            })
            .unwrap()
            .unwrap()
            .unwrap();

        let records = Cell::new(0);
        assert_eq!(
            consume_software_keyboard_success_with(
                SoftwareKeyboardCallbackProvenance::OwnedNull,
                || unsafe { complete_software_keyboard_job_success(0, 0) },
                || {
                    records.set(records.get() + 1);
                    coordinator.record_result(
                        JOB,
                        "must-not-publish",
                        PATH_EDITOR_STATUS_NATIVE_ACCEPT,
                    )
                },
            ),
            SoftwareKeyboardSuccessDisposition::Incomplete
        );
        assert_eq!(records.get(), 0);
        assert!(coordinator.recognizes_job(JOB));
        assert_eq!(coordinator.take_completed(test_identity(DIALOG)), Ok(None));

        let completions = Cell::new(0);
        assert_eq!(
            consume_software_keyboard_success_with(
                SoftwareKeyboardCallbackProvenance::UnreadableOwned,
                || {
                    completions.set(completions.get() + 1);
                    true
                },
                || {
                    records.set(records.get() + 1);
                    er_save_picker::PathEditorResultOwnership::Current
                },
            ),
            SoftwareKeyboardSuccessDisposition::Incomplete
        );
        assert_eq!((completions.get(), records.get()), (0, 0));
    }

    #[test]
    fn continuation_suppresses_owned_and_evicted_null_but_forwards_same_address_foreign() {
        use std::cell::Cell;

        let consumed = Cell::new(0);
        let forwards = Cell::new(0);
        assert_eq!(
            dispatch_software_keyboard_accept_continuation_with(
                SoftwareKeyboardCallbackProvenance::OwnedNull,
                || consumed.set(consumed.get() + 1),
                || {
                    forwards.set(forwards.get() + 1);
                    0xff
                },
                0xa1,
            ),
            0xa1
        );
        assert_eq!((consumed.get(), forwards.get()), (1, 0));

        assert_eq!(
            dispatch_software_keyboard_accept_continuation_with(
                SoftwareKeyboardCallbackProvenance::OrphanedNull,
                || {
                    consumed.set(consumed.get() + 1);
                    assert_eq!(
                        consume_software_keyboard_success_with(
                            SoftwareKeyboardCallbackProvenance::OrphanedNull,
                            || true,
                            || panic!("evicted null callback must never publish"),
                        ),
                        SoftwareKeyboardSuccessDisposition::Orphaned
                    );
                },
                || {
                    forwards.set(forwards.get() + 1);
                    0xff
                },
                0xb2,
            ),
            0xb2
        );
        assert_eq!((consumed.get(), forwards.get()), (2, 0));

        assert_eq!(
            dispatch_software_keyboard_accept_continuation_with(
                software_keyboard_callback_provenance(true, Some(0x1234)),
                || panic!("same-address foreign continuation must not be consumed"),
                || {
                    forwards.set(forwards.get() + 1);
                    0xc3
                },
                0xff,
            ),
            0xc3
        );
        assert_eq!((consumed.get(), forwards.get()), (2, 1));
    }

    #[test]
    fn lifecycle_status_values_are_distinct_and_idle_is_zero() {
        let statuses = [
            PATH_EDITOR_STATUS_IDLE,
            PATH_EDITOR_STATUS_PENDING,
            PATH_EDITOR_STATUS_SUBMITTED,
            PATH_EDITOR_STATUS_NATIVE_ACCEPT,
            PATH_EDITOR_STATUS_NATIVE_CANCEL,
            PATH_EDITOR_STATUS_STALE_RESULT,
            PATH_EDITOR_STATUS_IDENTITY_REJECTED,
            PATH_EDITOR_STATUS_SUBMIT_FAILED,
            PATH_EDITOR_STATUS_VALIDATION_REJECTED,
            PATH_EDITOR_STATUS_APPLIED_DIRECTORY,
            PATH_EDITOR_STATUS_REBUILD_SCHEDULED,
            PATH_EDITOR_STATUS_RESET,
            PATH_EDITOR_STATUS_RESET_DEFERRED,
            PathEditorLifecycleStatus::DeferredCloseDrained,
            PathEditorLifecycleStatus::DeferredCloseCancelled,
            PathEditorLifecycleStatus::Submitting,
            PathEditorLifecycleStatus::RecipeUnavailable,
        ];
        assert_eq!(statuses[0] as usize, 0);
        for (index, status) in statuses.iter().enumerate() {
            assert!(!statuses[..index].contains(status));
        }
    }

    #[test]
    fn enter_name_signature_is_the_exact_1162_rex_prefixed_prologue() {
        use iced_x86::{Decoder, DecoderOptions, Mnemonic, Register};

        assert_eq!(SOFTWARE_KEYBOARD_ENTER_NAME_RVA, 0xe70c00);
        assert_eq!(
            SOFTWARE_KEYBOARD_ENTER_NAME_SIG,
            &[0x40, 0x55, 0x56, 0x57, 0x48, 0x83, 0xec, 0x70]
        );
        let mut decoder = Decoder::with_ip(
            64,
            SOFTWARE_KEYBOARD_ENTER_NAME_SIG,
            0x140e70c00,
            DecoderOptions::NONE,
        );
        let instructions = (0..4).map(|_| decoder.decode()).collect::<Vec<_>>();
        assert_eq!(
            instructions
                .iter()
                .map(|instruction| instruction.mnemonic())
                .collect::<Vec<_>>(),
            [
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Push,
                Mnemonic::Sub
            ]
        );
        assert_eq!(instructions[0].op0_register(), Register::RBP);
        assert_eq!(instructions[1].op0_register(), Register::RSI);
        assert_eq!(instructions[2].op0_register(), Register::RDI);
        assert_eq!(instructions[3].op0_register(), Register::RSP);
        assert_eq!(instructions[3].immediate8(), 0x70);
        assert_eq!(
            instructions
                .iter()
                .map(|instruction| instruction.len())
                .collect::<Vec<_>>(),
            [2, 1, 1, 4]
        );
        assert!(!decoder.can_decode());
    }

    #[test]
    fn all_software_keyboard_recipe_fields_resolve_through_fake_verifiers() {
        const BASE: usize = 0x140000000;
        let mut verified = Vec::new();
        let mut resolved = Vec::new();
        let recipe = resolve_software_keyboard_recipe_with(
            |rva, signature, label| {
                verified.push((rva, signature.to_vec(), label));
                Some(BASE + rva as usize)
            },
            |rva| {
                resolved.push(rva);
                Some(BASE + rva as usize)
            },
        )
        .unwrap();

        assert_eq!(verified.len(), 7);
        assert!(verified.iter().any(|(rva, signature, label)| {
            *rva == SOFTWARE_KEYBOARD_ENTER_NAME_RVA
                && signature == SOFTWARE_KEYBOARD_ENTER_NAME_SIG
                && *label == "SoftwareKeyboard EnterName preset"
        }));
        assert_eq!(resolved, [MENU_JOB_QUEUE_READY_RVA, MENU_JOB_SUBMIT_RVA]);
        assert_eq!(recipe.ctor, BASE + SOFTWARE_KEYBOARD_JOB_CTOR_RVA as usize);
        assert_eq!(
            recipe.validator_init,
            BASE + SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA as usize
        );
        assert_eq!(
            recipe.validator_dtor,
            BASE + SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA as usize
        );
        assert_eq!(
            recipe.enter_name,
            BASE + SOFTWARE_KEYBOARD_ENTER_NAME_RVA as usize
        );
        assert_eq!(
            recipe.set_initial,
            BASE + SOFTWARE_KEYBOARD_SET_INITIAL_RVA as usize
        );
        assert_eq!(
            recipe.set_max,
            BASE + SOFTWARE_KEYBOARD_SET_MAX_RVA as usize
        );
        assert_eq!(recipe.heap_alloc, BASE + GAME_HEAP_ALLOC_RVA);
        assert_eq!(recipe.queue_ready, BASE + MENU_JOB_QUEUE_READY_RVA as usize);
        assert_eq!(recipe.submit, BASE + MENU_JOB_SUBMIT_RVA as usize);
    }

    fn test_identity(dialog: usize) -> er_save_picker::PathEditorPickerIdentity {
        er_save_picker::PathEditorPickerIdentity {
            picker_mode_active: true,
            current_dialog: dialog,
        }
    }

    #[test]
    fn cached_recipe_failure_is_one_terminal_rejection_without_tick_retries() {
        const DIALOG: usize = 0x5000;
        let coordinator = er_save_picker::PathEditorCoordinator::<
            (),
            er_save_picker::NoopPathEditorLifecyclePublisher,
        >::new(er_save_picker::NoopPathEditorLifecyclePublisher);
        coordinator.request(test_identity(DIALOG), DIALOG).unwrap();
        let cached_recipe = OnceLock::<Option<()>>::new();
        let mut events = Vec::new();
        let mut submit_calls = 0;

        let first = pump_path_editor_submit_with(
            &coordinator,
            test_identity(DIALOG),
            |_, _| {
                submit_calls += 1;
                dispatch_recipe_submit_with(
                    cached_recipe.get_or_init(|| None).as_ref(),
                    |_| panic!("queue readiness called without a recipe"),
                    |_| panic!("submit called without a recipe"),
                )
            },
            |event| events.push(event),
        )
        .unwrap();
        assert_eq!(first, Some(PathEditorSubmitDisposition::RecipeUnavailable));
        assert!(cached_recipe.get().is_some_and(Option::is_none));
        assert_eq!(
            pump_path_editor_submit_with(
                &coordinator,
                test_identity(DIALOG),
                |_, _| {
                    submit_calls += 1;
                    PathEditorSubmitDisposition::Submitted
                },
                |event| events.push(event),
            )
            .unwrap(),
            None
        );

        assert_eq!(submit_calls, 1);
        assert_eq!(
            events,
            [
                PathEditorSubmitEvent::Attempt,
                PathEditorSubmitEvent::Failure,
                PathEditorSubmitEvent::RecipeUnavailableRejection,
            ]
        );
        let snapshot = coordinator.snapshot();
        assert!(!snapshot.pending);
        assert!(!snapshot.submit_lease_active);
        assert_eq!(
            snapshot.status,
            PathEditorLifecycleStatus::RecipeUnavailable
        );
    }

    #[test]
    fn queue_not_ready_stays_pending_and_retryable() {
        const DIALOG: usize = 0x5000;
        let coordinator = er_save_picker::PathEditorCoordinator::<
            (),
            er_save_picker::NoopPathEditorLifecyclePublisher,
        >::new(er_save_picker::NoopPathEditorLifecyclePublisher);
        coordinator.request(test_identity(DIALOG), DIALOG).unwrap();
        let recipe = ();
        let mut queue_checks = 0;
        let mut submit_calls = 0;
        let mut events = Vec::new();

        for _ in 0..2 {
            assert_eq!(
                pump_path_editor_submit_with(
                    &coordinator,
                    test_identity(DIALOG),
                    |_, _| {
                        dispatch_recipe_submit_with(
                            Some(&recipe),
                            |_| {
                                queue_checks += 1;
                                false
                            },
                            |_| {
                                submit_calls += 1;
                                true
                            },
                        )
                    },
                    |event| events.push(event),
                )
                .unwrap(),
                Some(PathEditorSubmitDisposition::Retryable)
            );
        }

        assert_eq!(queue_checks, 2);
        assert_eq!(submit_calls, 0);
        assert_eq!(
            events,
            [
                PathEditorSubmitEvent::Attempt,
                PathEditorSubmitEvent::Failure,
                PathEditorSubmitEvent::Attempt,
                PathEditorSubmitEvent::Failure,
            ]
        );
        let snapshot = coordinator.snapshot();
        assert!(snapshot.pending);
        assert!(!snapshot.submit_lease_active);
        assert_eq!(snapshot.status, PathEditorLifecycleStatus::SubmitFailed);
    }

    #[test]
    fn resolved_ready_recipe_reaches_submit_and_activates_the_job() {
        const DIALOG: usize = 0x5000;
        const JOB: usize = 0x7000;
        let coordinator = er_save_picker::PathEditorCoordinator::<
            (),
            er_save_picker::NoopPathEditorLifecyclePublisher,
        >::new(er_save_picker::NoopPathEditorLifecyclePublisher);
        coordinator.request(test_identity(DIALOG), DIALOG).unwrap();
        let recipe = ();
        let mut submit_calls = 0;
        let mut events = Vec::new();

        assert_eq!(
            pump_path_editor_submit_with(
                &coordinator,
                test_identity(DIALOG),
                |ticket, coordinator| {
                    dispatch_recipe_submit_with(
                        Some(&recipe),
                        |_| true,
                        |_| {
                            submit_calls += 1;
                            coordinator.activate(ticket, JOB).unwrap();
                            true
                        },
                    )
                },
                |event| events.push(event),
            )
            .unwrap(),
            Some(PathEditorSubmitDisposition::Submitted)
        );
        assert_eq!(submit_calls, 1);
        assert_eq!(
            events,
            [
                PathEditorSubmitEvent::Attempt,
                PathEditorSubmitEvent::Success
            ]
        );
        let snapshot = coordinator.snapshot();
        assert!(!snapshot.pending);
        assert!(!snapshot.submit_lease_active);
        assert_eq!(snapshot.status, PathEditorLifecycleStatus::Submitted);
        assert!(coordinator.recognizes_job(JOB));
    }
}
