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
const SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_RVA: u32 = 0x81d220;
const SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_SIG: &[u8] = &[
    0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
];
const FD4_TIME_VTABLE_RVA: usize = 0x29c8e58;
const FD4_TIME_FLOAT_VTABLE_RVA: usize = 0x29c8e48;
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA: u32 = 0xe70920;
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30];
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA: u32 = 0xe70960;
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30];
const SOFTWARE_KEYBOARD_ENTER_NAME_RVA: u32 = 0xe70c00;
const SOFTWARE_KEYBOARD_ENTER_NAME_SIG: &[u8] = &[0x40, 0x55, 0x56, 0x57, 0x48, 0x83, 0xec];
const SOFTWARE_KEYBOARD_SET_INITIAL_RVA: u32 = 0xe709f0;
const SOFTWARE_KEYBOARD_SET_INITIAL_SIG: &[u8] =
    &[0x40, 0x55, 0x56, 0x57, 0x48, 0x8d, 0x6c, 0x24, 0xb9];
const SOFTWARE_KEYBOARD_SET_MAX_RVA: u32 = 0x2416ee0;
const SOFTWARE_KEYBOARD_SET_MAX_SIG: &[u8] =
    &[0xb8, 0x01, 0x00, 0x00, 0x00, 0x3b, 0xd0, 0x0f, 0x4d, 0xc2];
const GAME_HEAP_ALLOC_SIG: &[u8] = &[0x49, 0x8b, 0x00, 0x4d, 0x8b, 0xc8, 0x4c, 0x8b, 0xc2];
const GLOBAL_MENU_HEAP_ALLOCATOR_RVA: usize = 0x3d87350;

const SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET: usize = 0xd8;
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

/// Distinct Scaleform cache key for the path editor. The file-open hook redirects this miss to the
/// canonical 02_990 bytes and derives an inline movie without mutating the game's shared native
/// text-input resource.
const TEXT_INPUT_RESOURCE: [u16; 28] = [
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
    b'_' as u16,
    b'P' as u16,
    b'a' as u16,
    b't' as u16,
    b'h' as u16,
    b'E' as u16,
    b'd' as u16,
    b'i' as u16,
    b't' as u16,
    b'o' as u16,
    b'r' as u16,
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

#[derive(Debug)]
enum PathEditorOutcome {
    Accepted(String),
    Cancelled,
    TextUnreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathEditorSubmit {
    Submitted,
    RetryWhenQueueReady,
    Rejected,
}

static SOFTWARE_KEYBOARD_RECIPE: OnceLock<Option<SoftwareKeyboardRecipe>> = OnceLock::new();
static SOFTWARE_KEYBOARD_RESULT_GATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_WINDOW: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_OUTCOME: OnceLock<Mutex<Option<PathEditorOutcome>>> =
    OnceLock::new();

pub(crate) fn save_picker_path_editor_active() -> bool {
    SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst) != 0
        || SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.load(Ordering::SeqCst) != 0
}

/// Called only from the owned 02_990 MenuWindowJob::Run post-hook. A terminal MenuWindow result
/// means its SceneObjProxy teardown has begun: never write another transform through that proxy.
fn path_editor_window_is_live(state: i32) -> bool {
    // A newly-created MenuWindow begins at zero before its first controller update. Continue is 1;
    // only Success/Failed are terminal. Treating zero as terminal cancelled every editor during its
    // construction frame and queued a ProfileSelect rebuild against half-bound child components.
    state == 0 || state == MENU_JOB_STATE_CONTINUE
}

pub(crate) fn save_picker_note_path_editor_window_state(window: usize, state: i32) -> bool {
    if path_editor_window_is_live(state) {
        let previous_window = SAVE_PICKER_PATH_EDITOR_WINDOW.swap(window, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.store(
            er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if previous_window == 0 {
            // A fresh editor: re-arm the end-caret. This transition is the only per-open signal --
            // the window pointer itself gets recycled across opens.
            reset_path_editor_caret_latch();
            let dialog = SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.load(Ordering::SeqCst);
            if dialog != 0
                && SAVE_PICKER_REBUILD_PENDING_DIALOG
                    .compare_exchange(0, dialog, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                append_autoload_debug(format_args!(
                    "save-picker-path: 02_990 became visible window=0x{window:x}; queued ProfileSelect row rebuild to hide read-only CurrentPath"
                ));
            }
        }
        return true;
    }
    if SAVE_PICKER_PATH_EDITOR_WINDOW.load(Ordering::SeqCst) == window {
        SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
    }
    let active = SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst);
    if active != 0
        && SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB
            .compare_exchange(active, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        *path_editor_outcome()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathEditorOutcome::Cancelled);
        append_autoload_debug(format_args!(
            "save-picker-path: 02_990 MenuWindow became terminal state={state} window=0x{window:x}; released job=0x{active:x} as cancelled before proxy teardown"
        ));
    }
    false
}

fn path_editor_outcome() -> &'static Mutex<Option<PathEditorOutcome>> {
    SAVE_PICKER_PATH_EDITOR_OUTCOME.get_or_init(|| Mutex::new(None))
}

/// Drop every pointer/queued result owned by a ProfileSelect path-editor session. Called only after
/// the native ProfileSelect MenuWindow finalizer has run; retaining any of these values lets a later
/// `Load Character from File` reuse a dead dialog/job from the prior menu generation.
pub(crate) fn save_picker_reset_path_editor_state() {
    SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.store(0, Ordering::SeqCst);
    *path_editor_outcome()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn software_keyboard_recipe() -> Option<&'static SoftwareKeyboardRecipe> {
    SOFTWARE_KEYBOARD_RECIPE
        .get_or_init(|| {
            Some(SoftwareKeyboardRecipe {
                ctor: save_flow_verify_rva(
                    SOFTWARE_KEYBOARD_JOB_CTOR_RVA,
                    SOFTWARE_KEYBOARD_JOB_CTOR_SIG,
                    "SoftwareKeyboardJob ctor",
                )?,
                validator_init: save_flow_verify_rva(
                    SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA,
                    SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG,
                    "SoftwareKeyboard validator init",
                )?,
                validator_dtor: save_flow_verify_rva(
                    SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA,
                    SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG,
                    "SoftwareKeyboard validator dtor",
                )?,
                enter_name: save_flow_verify_rva(
                    SOFTWARE_KEYBOARD_ENTER_NAME_RVA,
                    SOFTWARE_KEYBOARD_ENTER_NAME_SIG,
                    "SoftwareKeyboard EnterName preset",
                )?,
                set_initial: save_flow_verify_rva(
                    SOFTWARE_KEYBOARD_SET_INITIAL_RVA,
                    SOFTWARE_KEYBOARD_SET_INITIAL_SIG,
                    "SoftwareKeyboard initial text setter",
                )?,
                set_max: save_flow_verify_rva(
                    SOFTWARE_KEYBOARD_SET_MAX_RVA,
                    SOFTWARE_KEYBOARD_SET_MAX_SIG,
                    "SoftwareKeyboard max-length setter",
                )?,
                heap_alloc: save_flow_verify_rva(
                    GAME_HEAP_ALLOC_RVA as u32,
                    GAME_HEAP_ALLOC_SIG,
                    "game heap allocator",
                )?,
                queue_ready: game_rva(MENU_JOB_QUEUE_READY_RVA).ok()?,
                submit: game_rva(MENU_JOB_SUBMIT_RVA).ok()?,
            })
        })
        .as_ref()
}

fn install_software_keyboard_result_hooks() -> bool {
    if SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED.load(Ordering::SeqCst) == 1
        && SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED.load(Ordering::SeqCst) == 1
    {
        // The live prologues are now MinHook jumps, so signature verification is valid only before
        // first installation. Reuse the already-verified hooks on every later editor open.
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
        "SoftwareKeyboard path cancel gate",
    );
    let Some(terminal) = save_flow_verify_rva(
        SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_RVA,
        SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_SIG,
        "SoftwareKeyboard terminal callback",
    ) else {
        return false;
    };
    mh_install_hook_once(
        &SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED,
        0,
        1,
        terminal,
        software_keyboard_terminal_callback_hook as *mut c_void,
        &SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_ORIG,
        "SoftwareKeyboard path terminal callback",
    );
    SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED.load(Ordering::SeqCst) == 1
        && SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_INSTALLED.load(Ordering::SeqCst) == 1
}

unsafe fn software_keyboard_result_state(job: usize) -> Option<i32> {
    let controller = unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET) }?;
    if controller == 0 || controller == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    unsafe { safe_read_i32(controller + SOFTWARE_KEYBOARD_CONTROLLER_RESULT_78_OFFSET) }
}

const NATIVE_PATH_EDITOR_BACKSLASH_SENTINEL: char = '\u{3254}';

/// The native 02_990 controller rewrites every Windows backslash in its accepted DLString to U+3254
/// (runtime-captured on unchanged `Z:\\...` paths). Decode that transport sentinel before validating
/// the path; otherwise an untouched valid absolute path is rejected as relative.
fn normalize_native_path_editor_text(text: String) -> String {
    text.replace(NATIVE_PATH_EDITOR_BACKSLASH_SENTINEL, "\\")
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
    if SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst) != job {
        return unsafe { original(job, result, time) };
    }

    // Preserve the native accepted-state and intermediate cleanup chain. The owned terminal d220
    // detour below replaces only the callback leaf that our intentionally-empty std::function cannot
    // satisfy. Cancellation still terminates here and never reaches d220.
    let ret = unsafe { original(job, result, time) };
    let result_state = unsafe { safe_read_i32(result) }.unwrap_or(0);
    if result_state == MENU_JOB_STATE_FAILED {
        *path_editor_outcome()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathEditorOutcome::Cancelled);
        SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-path: native SoftwareKeyboard cancelled job=0x{job:x}; model remains unchanged"
        ));
    }
    ret
}

fn path_editor_owns_terminal_job(active_job: usize, job: usize) -> bool {
    active_job != 0 && active_job == job
}

unsafe extern "system" fn software_keyboard_terminal_callback_hook(
    job: usize,
    result: usize,
    time: usize,
) -> usize {
    let original_addr = SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_ORIG.load(Ordering::SeqCst);
    if original_addr == HOOK_ORIGINAL_UNSET {
        return result;
    }
    let active_job = SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst);
    if !path_editor_owns_terminal_job(active_job, job) {
        let original: unsafe extern "system" fn(usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(original_addr) };
        return unsafe { original(job, result, time) };
    }

    let outcome = match unsafe { software_keyboard_text(job) } {
        Some(raw_text) => {
            let text = normalize_native_path_editor_text(raw_text.clone());
            append_autoload_debug(format_args!(
                "save-picker-path: native editor accepted raw={raw_text:?} normalized={text:?} utf16_units={}",
                text.encode_utf16().count()
            ));
            PathEditorOutcome::Accepted(text)
        }
        None => PathEditorOutcome::TextUnreadable,
    };
    *path_editor_outcome()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);
    SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.store(0, Ordering::SeqCst);

    // Exact native d220 tail after callback return: SetResult(Success, 0), then restore FD4Time's
    // two vtables in order. This skips only the null job+0x1a0 callback invocation.
    if result != 0 && result != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
    }
    if time != 0
        && time != TITLE_OWNER_SCAN_START_ADDRESS
        && let Ok(base) = game_module_base()
    {
        unsafe {
            *(time as *mut usize) = base + FD4_TIME_VTABLE_RVA;
            *(time as *mut usize) = base + FD4_TIME_FLOAT_VTABLE_RVA;
        }
    }
    append_autoload_debug(format_args!(
        "save-picker-path: native SoftwareKeyboard terminal accepted job=0x{job:x}; captured exact UTF-16 path and skipped empty callback"
    ));
    result
}

pub(crate) fn save_picker_request_path_editor(dialog: usize) {
    if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
        let cleared_stale_warning = crate::experiments::save_picker::active_save_picker_lock()
            .as_mut()
            .is_some_and(er_save_picker::SavePickerModel::begin_path_edit);
        SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-path: activation requested dialog=0x{dialog:x} cleared_stale_warning={cleared_stale_warning}"
        ));
    }
}

unsafe fn submit_path_editor(dialog: usize) -> PathEditorSubmit {
    if SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst) != 0 {
        return PathEditorSubmit::RetryWhenQueueReady;
    }
    let Some(recipe) = software_keyboard_recipe() else {
        append_autoload_debug(format_args!(
            "save-picker-path: native SoftwareKeyboard recipe unavailable; refusing unsafe call"
        ));
        return PathEditorSubmit::Rejected;
    };
    if !install_software_keyboard_result_hooks() {
        append_autoload_debug(format_args!(
            "save-picker-path: result hooks unavailable; refusing a job whose terminal callback cannot be captured safely"
        ));
        return PathEditorSubmit::Rejected;
    }

    let queue = dialog + SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET;
    let queue_ready: unsafe extern "system" fn(usize) -> u8 =
        unsafe { std::mem::transmute(recipe.queue_ready) };
    if unsafe { queue_ready(queue) } == 0 {
        return PathEditorSubmit::RetryWhenQueueReady;
    }
    let initial = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return PathEditorSubmit::Rejected;
        };
        let Some(path) = model.current_dir().to_str() else {
            return PathEditorSubmit::Rejected;
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
        return PathEditorSubmit::Rejected;
    };
    let allocator = match unsafe { safe_read_usize(base + GLOBAL_MENU_HEAP_ALLOCATOR_RVA) } {
        Some(allocator) if allocator != 0 && allocator != TITLE_OWNER_SCAN_START_ADDRESS => {
            allocator
        }
        _ => {
            unsafe { validator_dtor(validator_ptr) };
            return PathEditorSubmit::Rejected;
        }
    };
    let heap_alloc: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(recipe.heap_alloc) };
    let memory = unsafe { heap_alloc(SOFTWARE_KEYBOARD_JOB_SIZE, 8, allocator) };
    if memory == 0 || memory == TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { validator_dtor(validator_ptr) };
        return PathEditorSubmit::Rejected;
    }

    let config = SoftwareKeyboardConfig {
        max_units: SOFTWARE_KEYBOARD_MAX_PATH_UNITS as u32,
        mode: 1,
        padding: [0; 3],
        resource: TEXT_INPUT_RESOURCE.as_ptr(),
    };
    let empty_callback = [0_usize; 8];
    let ctor: unsafe extern "system" fn(usize, usize, usize, usize, usize, u8, usize) -> usize =
        unsafe { std::mem::transmute(recipe.ctor) };
    let job = unsafe {
        ctor(
            memory,
            dialog + SYSTEM_QUIT_DIALOG_MENU_WINDOW_LIST_50_OFFSET,
            validator_ptr,
            (&raw const config) as usize,
            initial.as_ptr() as usize,
            1,
            empty_callback.as_ptr() as usize,
        )
    };
    unsafe { validator_dtor(validator_ptr) };
    if job == 0 || job == TITLE_OWNER_SCAN_START_ADDRESS {
        return PathEditorSubmit::Rejected;
    }

    unsafe {
        let refcount = (job + MENU_JOB_REFCOUNT_08_OFFSET) as *mut std::sync::atomic::AtomicI32;
        (*refcount).fetch_add(1, Ordering::SeqCst);
    }
    SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
    SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.store(
        er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.store(job, Ordering::SeqCst);
    *path_editor_outcome()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let mut job_slot = job;
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.submit) };
    unsafe { submit(queue, (&raw mut job_slot) as usize) };
    append_autoload_debug(format_args!(
        "save-picker-path: submitted native SoftwareKeyboardJob=0x{job:x} dialog=0x{dialog:x} queue=0x{queue:x} initial_units={} max_units={SOFTWARE_KEYBOARD_MAX_PATH_UNITS}",
        initial.len().saturating_sub(1)
    ));
    PathEditorSubmit::Submitted
}

fn apply_path_editor_outcome(dialog: usize, outcome: PathEditorOutcome) {
    let mut guard = crate::experiments::save_picker::active_save_picker_lock();
    let Some(model) = guard.as_mut() else {
        return;
    };
    match outcome {
        PathEditorOutcome::Accepted(path) => match model.set_current_dir_from_text(&path) {
            Ok(changed) => append_autoload_debug(format_args!(
                "save-picker-path: committed accepted directory changed={changed} exact='{}'",
                model.current_dir().display()
            )),
            Err(reason) => {
                model.set_status_message(reason.status_message());
                // Keep what was typed on the control, marked invalid, so it can be corrected in
                // place. Ordering matters: `set_status_message` does not clear the rejected text,
                // while a later valid entry refreshes the listing and drops it automatically.
                model.set_rejected_path_text(&path);
                append_autoload_debug(format_args!(
                    "save-picker-path: rejected accepted text reason={reason:?}; keeping '{path}' on the control as invalid; directory remains '{}'",
                    model.current_dir().display()
                ));
            }
        },
        PathEditorOutcome::Cancelled => {
            append_autoload_debug(format_args!(
                "save-picker-path: cancel consumed; directory remains '{}'",
                model.current_dir().display()
            ));
        }
        PathEditorOutcome::TextUnreadable => {
            model.set_status_message(er_save_picker::PickerStatusMessage::new(
                "PATH TEXT UNREADABLE",
                "The native editor returned invalid UTF-16; the folder was not changed.",
            ));
        }
    }
    if unsafe { save_picker_stage_row_records(model) } {
        SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
    }
}

/// Menu-pump-owned submit/result bridge. The native text editor and its job queue are never touched
/// from FrameBegin or the recurring game task.
pub(crate) unsafe fn save_picker_menu_pump_path_editor() {
    let active_before_watchdog = SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst);
    let editor_window = SAVE_PICKER_PATH_EDITOR_WINDOW.load(Ordering::SeqCst);
    if active_before_watchdog != 0 && editor_window != 0 {
        let last = SAVE_PICKER_PATH_EDITOR_WINDOW_LAST_PROFILE_TICK.load(Ordering::SeqCst);
        let now = er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst);
        if now.saturating_sub(last) >= PATH_EDITOR_WINDOW_STALE_PROFILE_TICKS
            && SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB
                .compare_exchange(
                    active_before_watchdog,
                    0,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
            *path_editor_outcome()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(PathEditorOutcome::Cancelled);
            append_autoload_debug(format_args!(
                "save-picker-path: 02_990 MenuWindow stopped running for {} ProfileSelect ticks; released stale job=0x{active_before_watchdog:x} window=0x{editor_window:x} as cancelled before reading freed controller state",
                now.saturating_sub(last)
            ));
        }
    }

    let active_job = SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst);
    if active_job != 0
        && unsafe { software_keyboard_result_state(active_job) } == Some(MENU_JOB_STATE_FAILED)
        && SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB
            .compare_exchange(active_job, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        *path_editor_outcome()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathEditorOutcome::Cancelled);
        append_autoload_debug(format_args!(
            "save-picker-path: observed native SoftwareKeyboard failed/cancelled state for job=0x{active_job:x}; released editor ownership"
        ));
    }

    let outcome = path_editor_outcome()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(outcome) = outcome {
        let dialog = SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.swap(0, Ordering::SeqCst);
        SAVE_PICKER_PATH_EDITOR_WINDOW.store(0, Ordering::SeqCst);
        if dialog != 0 {
            apply_path_editor_outcome(dialog, outcome);
        }
    }

    if SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst) != 0 {
        return;
    }
    let dialog = SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.load(Ordering::SeqCst);
    if dialog == 0 {
        return;
    }
    match unsafe { submit_path_editor(dialog) } {
        PathEditorSubmit::Submitted => {
            SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(0, Ordering::SeqCst);
            let guard = crate::experiments::save_picker::active_save_picker_lock();
            if let Some(model) = guard.as_ref()
                && unsafe { save_picker_stage_row_records(model) }
            {
                SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
            }
        }
        PathEditorSubmit::RetryWhenQueueReady => {}
        PathEditorSubmit::Rejected => {
            SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(0, Ordering::SeqCst);
            let mut guard = crate::experiments::save_picker::active_save_picker_lock();
            if let Some(model) = guard.as_mut() {
                model.set_status_message(er_save_picker::PickerStatusMessage::new(
                    "PATH EDITOR UNAVAILABLE",
                    "The native text editor could not be opened; the current folder was not changed.",
                ));
                if unsafe { save_picker_stage_row_records(model) } {
                    SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
                }
            }
        }
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
            "02_990_TextInput_PathEditor"
        );
    }

    #[test]
    fn native_keyboard_separator_sentinel_decodes_to_a_windows_backslash() {
        assert_eq!(
            normalize_native_path_editor_text("Z:㉔home㉔banon㉔saves".to_owned()),
            r"Z:\home\banon\saves"
        );
    }

    #[test]
    fn terminal_capture_is_scoped_to_the_owned_software_keyboard_job() {
        assert!(path_editor_owns_terminal_job(0x1234, 0x1234));
        assert!(!path_editor_owns_terminal_job(0, 0));
        assert!(!path_editor_owns_terminal_job(0x1234, 0x5678));
        assert_eq!(SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET, 0xd8);
        assert_eq!(SOFTWARE_KEYBOARD_CONTROLLER_TEXT_80_OFFSET, 0x80);
        assert_eq!(FD4_TIME_VTABLE_RVA, 0x29c8e58);
        assert_eq!(FD4_TIME_FLOAT_VTABLE_RVA, 0x29c8e48);
    }

    #[test]
    fn terminal_window_states_are_never_transform_targets() {
        assert!(path_editor_window_is_live(0));
        assert!(path_editor_window_is_live(MENU_JOB_STATE_CONTINUE));
        assert!(!path_editor_window_is_live(MENU_JOB_STATE_SUCCESS));
        assert!(!path_editor_window_is_live(MENU_JOB_STATE_FAILED));
    }

    #[test]
    fn path_limit_exceeds_the_native_name_presets_without_becoming_unbounded() {
        assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS > 16);
        assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS <= 1024);
    }
}
