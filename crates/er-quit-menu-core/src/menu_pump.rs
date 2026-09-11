//! The menu pump the link field needs, for a host that has no other reason to own one.
//!
//! `CS::MenuWindowJob::Run` is the game's menu pump executing one window. Three things have to
//! happen inside it and nowhere else:
//!
//! * the field's `CS::SoftwareKeyboardJob` is built and submitted there. Building or submitting a
//!   `MenuJob` from a game task instead produced the Scaleform race behind the non-deterministic
//!   execute faults (bd `system-quit-return-title-scaleform-race`);
//! * the field's `SceneObjProxy` resolves are only valid while its own window is running, which is
//!   what the end-caret and the live clipboard mirror need;
//! * a window that stopped running is the only signal a field closed by the back action emits, so
//!   the abandoned-latch sweep has to run on the same cadence.
//!
//! The product drives all three from its own post-run hook, which also owns the save picker, the
//! return-title chain and the in-world pause menu. A standalone shell has none of those, so this is
//! the narrow version: one detour, one resource name, and nothing else touched.

use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::mem::{game_module_base, game_rva_for_hook, safe_read_i32, safe_read_usize};

use crate::build_url_editor::{build_url_editor_menu_pump, build_url_editor_window_run};
use crate::host::append_autoload_debug;
use crate::scaleform_proxy::apply_build_url_editor_window_position;
use crate::software_keyboard::{
    BUILD_URL_TEXT_INPUT_RESOURCE_NAME, build_url_note_editor_window_state,
    text_input_02_990_window_is_live,
};

/// `CS::MenuWindowJob::Run`. Declared once in `er-title-flow`, where the product's own detour on the
/// same address reads it, and derived here.
const MENU_WINDOW_JOB_RUN_RVA: u32 = er_title_flow::PAB_NODE_UPDATE_RVA;
/// `MenuWindowJob.resourceName`, the `wchar_t*` the game itself uses to tell its windows apart.
const MENU_WINDOW_JOB_RESOURCE_NAME_60_OFFSET: usize = 0x60;
const MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET: usize =
    er_title_flow::MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET;
const MSGBOX_JOB_RESULT_STATE_1E8_OFFSET: usize = er_title_flow::MSGBOX_JOB_RESULT_STATE_1E8_OFFSET;

static RUN_ORIG: AtomicUsize = AtomicUsize::new(0);
static RUN_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Read a NUL-terminated UTF-16 resource name, bounded.
fn read_wide_resource_name(ptr: usize) -> String {
    const MAX_UNITS: usize = 64;
    if ptr < 0x10000 {
        return String::new();
    }
    let mut units = Vec::new();
    for idx in 0..MAX_UNITS {
        // Safety: the fault-safe reader answers `None` rather than faulting on a wild pointer.
        let unit = unsafe { er_game_base::mem::safe_read_u16(ptr + idx * 2) }.unwrap_or(0);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    // UTF-8 Lossy: a resource name is the game's own ASCII identifier; an unpaired surrogate here
    // would mean the pointer was not a resource name at all, and the comparison below then fails,
    // which is the right answer.
    String::from_utf16_lossy(&units)
}

/// Post-run work for the link field's own window, plus the pump step that submits a queued field.
///
/// # Safety
///
/// Installed by `er-hook`; the game calls it on its menu thread with a live `MenuWindowJob`.
unsafe extern "system" fn quit_menu_window_job_run_hook(
    job: usize,
    a: usize,
    b: usize,
    c: usize,
) -> usize {
    let orig = RUN_ORIG.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    // Safety: the union publishes either the game trampoline or the next handler in the chain,
    // both of which take four registers under the union signature.
    let next: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
    let ret = unsafe { next(job, a, b, c) };

    let filename_ptr =
        unsafe { safe_read_usize(job + MENU_WINDOW_JOB_RESOURCE_NAME_60_OFFSET) }.unwrap_or(0);
    if read_wide_resource_name(filename_ptr) == BUILD_URL_TEXT_INPUT_RESOURCE_NAME {
        let owner =
            unsafe { safe_read_usize(job + MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET) }.unwrap_or(0);
        if owner != 0 {
            let state = unsafe { safe_read_i32(owner + MSGBOX_JOB_RESULT_STATE_1E8_OFFSET) }
                .unwrap_or_default();
            if build_url_note_editor_window_state(owner, state)
                && let Ok(base) = game_module_base()
            {
                unsafe { apply_build_url_editor_window_position(base, owner) };
                // A window is only worth touching while it is still running; a terminal result
                // means its `SceneObjProxy` teardown has begun and a resolve would hand back
                // released objects.
                if text_input_02_990_window_is_live(state) {
                    unsafe { build_url_editor_window_run(base, owner) };
                }
            }
        }
    }
    // Menu-pump-owned: this is where a queued field is submitted and a finished one consumed.
    // Safety: this hook is the menu pump.
    unsafe { build_url_editor_menu_pump() };
    ret
}

/// Detour `MenuWindowJob::Run` so the link field has a menu pump.
///
/// Only a host that has no pump of its own calls this. The product does not: its own post-run work
/// already drives the field, from a detour it installs on the same address for the save picker and
/// the return-title chain as well.
///
/// # Safety
///
/// Process attach or startup-hook context.
pub unsafe fn install_quit_menu_window_run_hook() -> bool {
    if RUN_INSTALLED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return true;
    }
    let Ok(addr) = game_rva_for_hook(MENU_WINDOW_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-build-url: failed to resolve MenuWindowJob::Run rva 0x{MENU_WINDOW_JOB_RUN_RVA:x}; the link field will open and then never be driven"
        ));
        RUN_INSTALLED.store(0, Ordering::SeqCst);
        return false;
    };
    match unsafe { er_hook::register_union_hook(addr, quit_menu_window_job_run_hook, &RUN_ORIG) } {
        Ok(()) => {
            append_autoload_debug(format_args!(
                "system-quit-build-url: registered MenuWindowJob::Run 0x{addr:x} on the union; the link field has a menu pump"
            ));
            true
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "system-quit-build-url: register_union_hook MenuWindowJob::Run failed: {status:?}; the link field will open and then never be driven"
            ));
            RUN_INSTALLED.store(0, Ordering::SeqCst);
            false
        }
    }
}
