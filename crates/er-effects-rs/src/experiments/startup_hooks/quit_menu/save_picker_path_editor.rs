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
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_RVA: u32 = 0xe70920;
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30];
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_RVA: u32 = 0xe70960;
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG: &[u8] =
    &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30];
const SOFTWARE_KEYBOARD_ENTER_NAME_RVA: u32 = 0xe70c00;
const SOFTWARE_KEYBOARD_ENTER_NAME_SIG: &[u8] = &[0x55, 0x56, 0x57, 0x48, 0x83, 0xec, 0x70];
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
const MENU_JOB_STATE_SUCCESS: i32 = 2;
const MENU_JOB_STATE_FAILED: i32 = 3;

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

#[derive(Debug)]
enum PathEditorOutcome {
    Accepted(String),
    Cancelled,
    TextUnreadable,
}

static SOFTWARE_KEYBOARD_RECIPE: OnceLock<Option<SoftwareKeyboardRecipe>> = OnceLock::new();
static SOFTWARE_KEYBOARD_RESULT_GATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SOFTWARE_KEYBOARD_RESULT_GATE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PATH_EDITOR_OUTCOME: OnceLock<Mutex<Option<PathEditorOutcome>>> =
    OnceLock::new();

fn path_editor_outcome() -> &'static Mutex<Option<PathEditorOutcome>> {
    SAVE_PICKER_PATH_EDITOR_OUTCOME.get_or_init(|| Mutex::new(None))
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

    let controller =
        unsafe { safe_read_usize(job + SOFTWARE_KEYBOARD_JOB_CONTROLLER_D8_OFFSET) }.unwrap_or(0);
    let keyboard_state = if controller != 0 && controller != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_i32(controller + SOFTWARE_KEYBOARD_CONTROLLER_RESULT_78_OFFSET) }
            .unwrap_or(0)
    } else {
        0
    };
    if keyboard_state == MENU_JOB_STATE_SUCCESS {
        let outcome = match unsafe { software_keyboard_text(job) } {
            Some(text) => PathEditorOutcome::Accepted(text),
            None => PathEditorOutcome::TextUnreadable,
        };
        *path_editor_outcome()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(outcome);
        SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.store(0, Ordering::SeqCst);
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
        append_autoload_debug(format_args!(
            "save-picker-path: native SoftwareKeyboard accepted job=0x{job:x}; bypassed name validation and staged exact UTF-16 path text"
        ));
        return result;
    }

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

pub(crate) fn save_picker_request_path_editor(dialog: usize) {
    if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
        SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
    }
}

unsafe fn submit_path_editor(dialog: usize) -> bool {
    if SAVE_PICKER_PATH_EDITOR_ACTIVE_JOB.load(Ordering::SeqCst) != 0 {
        return false;
    }
    let Some(recipe) = software_keyboard_recipe() else {
        append_autoload_debug(format_args!(
            "save-picker-path: native SoftwareKeyboard recipe unavailable; refusing unsafe call"
        ));
        return false;
    };
    if !install_software_keyboard_result_gate_hook() {
        append_autoload_debug(format_args!(
            "save-picker-path: result gate hook unavailable; refusing a job whose callback cannot be captured safely"
        ));
        return false;
    }

    let queue = dialog + SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET;
    let queue_ready: unsafe extern "system" fn(usize) -> u8 =
        unsafe { std::mem::transmute(recipe.queue_ready) };
    if unsafe { queue_ready(queue) } == 0 {
        return false;
    }
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
        return false;
    }

    unsafe {
        let refcount = (job + MENU_JOB_REFCOUNT_08_OFFSET) as *mut std::sync::atomic::AtomicI32;
        (*refcount).fetch_add(1, Ordering::SeqCst);
    }
    SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.store(dialog, Ordering::SeqCst);
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
    true
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
                append_autoload_debug(format_args!(
                    "save-picker-path: rejected accepted text reason={reason:?}; directory remains '{}'",
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
    let outcome = path_editor_outcome()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(outcome) = outcome {
        let dialog = SAVE_PICKER_PATH_EDITOR_ACTIVE_DIALOG.swap(0, Ordering::SeqCst);
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
    if unsafe { submit_path_editor(dialog) } {
        SAVE_PICKER_PATH_EDITOR_PENDING_DIALOG.store(0, Ordering::SeqCst);
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
    fn path_limit_exceeds_the_native_name_presets_without_becoming_unbounded() {
        assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS > 16);
        assert!(SOFTWARE_KEYBOARD_MAX_PATH_UNITS <= 1024);
    }
}
