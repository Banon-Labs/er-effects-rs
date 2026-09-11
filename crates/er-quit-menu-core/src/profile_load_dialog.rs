//! Opening the game's own `05_010_ProfileSelect` character picker over a live System dialog.
//!
//! This is the entry point behind two of the cloned Quit rows. **Load Character** submits the
//! window over the container already loaded; **Load Character from File** stages a browse listing
//! into `CS::ProfileSummary` first and then submits the same window, so one opener serves both.
//!
//! # Why it is a submit and not a call
//!
//! `05_010_ProfileSelect` is a `CS::MenuWindowJob`, and the game's menu pump is the only thing
//! allowed to run one. The native wrapper at [`er_title_flow::PROFILE_SELECT_WRAPPER_RVA`] builds
//! the job into a caller-supplied slot; [`er_title_flow::MENU_JOB_SUBMIT_RVA`] hands that slot to a
//! queue. Both arguments come from the System dialog the row was pressed on:
//!
//! * `dialog + 0x10` is the dialog's own `MenuJobQueue`, so the submitted window is owned by the
//!   menu that opened it and is torn down with it;
//! * `dialog + 0x50` is the `DLFixedVector<8>` of owning `MenuWindow`s that `MenuWindowJob::Run`
//!   appends the loaded window to. Passing the `SceneObjProxy` back-reference here instead lets the
//!   resource load start and then asserts inside `DLFixedVector.inl` when `Run` appends to an
//!   object that is not that vector, which is why the count at `+0x48` is screened first.
//!
//! # What is checked before anything is called
//!
//! Every read is fault-safe and every refusal is a logged `false` rather than a guess, because the
//! two callers reach this from a menu-thread row press where a wrong pointer is a crash rather than
//! a wrong pixel:
//!
//! * the dialog is heap-like;
//! * its embedded `SceneObjProxy` carries the `CS::SceneObjProxy` vtable, which is what proves the
//!   pointer really is a System dialog and not another dialog at the same address;
//! * the owning-window vector has room;
//! * both native addresses resolve on the running build.

use core::sync::atomic::Ordering;

use er_game_base::mem::{game_module_base, game_rva, safe_read_usize};

use crate::host::{append_autoload_debug, maybe_build_profile_table_for_loading};

use er_telemetry_core::counters::{
    SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE, SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT,
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG, SYSTEM_QUIT_TOP_HIDE_ARMED_LIST,
};
use er_title_flow::{
    MENU_JOB_SUBMIT_RVA, PROFILE_MODEL_REND_TABLE_RVA, PROFILE_SELECT_WRAPPER_RVA,
    SCENE_OBJ_PROXY_VTABLE_RVA, SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET,
    SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET, SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG,
    TITLE_OWNER_SCAN_START_ADDRESS,
};

/// Lowest address this code will treat as a heap pointer. Anything below it is a refusal rather
/// than a read.
const HEAP_LO: usize = 0x10000;

/// The `DLFixedVector` at `dialog + 0x50` holds eight owning windows. A submit that would overflow
/// it asserts inside the game rather than failing, so a full vector refuses here.
const MENU_WINDOW_LIST_CAPACITY: usize = 8;

/// Open the character picker from a row's action object.
///
/// # Safety
///
/// Menu thread, with the action object the row router captured for this press.
pub unsafe fn system_quit_open_profile_load_dialog(action_obj: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let system_dialog =
        unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(NULL);
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- action=0x{action_obj:x} dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    unsafe { system_quit_open_profile_load_dialog_on(system_dialog) }
}

/// Submit the native `05_010_ProfileSelect` window against an already-resolved System/Quit
/// `PropertyEditDialog`. Split out of the action-object form (save-game-flow WP3) because the save
/// flow opens the destination browser from the dialog it captured at the row press -- it never has
/// a row action object of its own.
///
/// # Safety
///
/// Menu thread, with a live System dialog.
pub unsafe fn system_quit_open_profile_load_dialog_on(system_dialog: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- module base unavailable"
        ));
        return false;
    };
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    let scene_proxy = system_dialog + SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET;
    let scene_proxy_vt = unsafe { safe_read_usize(scene_proxy) }.unwrap_or(NULL);
    let want_scene_proxy_vt = er_game_base::mem::game_data_addr(
        base,
        SCENE_OBJ_PROXY_VTABLE_RVA,
        "SCENE_OBJ_PROXY_VTABLE_RVA",
    );
    if scene_proxy_vt != want_scene_proxy_vt {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- dialog=0x{system_dialog:x} scene_proxy=dialog+0x{SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET:x}=0x{scene_proxy:x} vt=0x{scene_proxy_vt:x} want=0x{want_scene_proxy_vt:x}"
        ));
        return false;
    }
    // Native title/menu route callers pass `owner + 0x50` as the MenuWindowJob's
    // field2_0x50 list argument. MenuWindowJob::Run later appends the loaded
    // owning MenuWindow to this DLFixedVector via FUN_140733ff0. Passing the
    // SceneObjProxy backref here is wrong: it lets the resource load start, then
    // asserts in DLFixedVector.inl line 0x296 when Run appends to a full/wrong
    // object.
    let menu_window_list = system_dialog + 0x50;
    let menu_window_list_count = unsafe { safe_read_usize(menu_window_list + 0x48) }.unwrap_or(!0);
    if menu_window_list_count >= MENU_WINDOW_LIST_CAPACITY {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- candidate menu_window_list=dialog+0x50=0x{menu_window_list:x} count@+0x48={menu_window_list_count} would overflow DLFixedVector<{MENU_WINDOW_LIST_CAPACITY}>"
        ));
        return false;
    }
    // The window this is about to submit renders character models, and the refresh that draws them
    // (`PROFILE_RENDERER_REFRESH_RVA`, 1.17.1 `0x1409ab820`) walks the profile model renderer table
    // at `PROFILE_MODEL_REND_TABLE_RVA` without checking it. When that table is null the refresh
    // reads `[null + 0x754]` and the process dies.
    //
    // Measured 2026-09-11, three runs on a shell-only profile: press Load Character, and the game
    // takes `0xc0000005` at `eldenring.exe+0x9ab874` reading `0x754`, with `rdi = *(0x143d71940) =
    // 0` in the crash record. The table is built by the loading-cover pipeline, which reaches this
    // crate only through `QuitMenuHost::maybe_build_profile_table_for_loading` -- a product-owned
    // entry whose neutral default does nothing. So a load with no product behind it submits a
    // window that cannot draw.
    //
    // Ask the host to build it before refusing: a product whose table is simply not up yet gets it
    // built and proceeds exactly as before, and only a load that has no way to produce one declines.
    // Declining costs the row a press; submitting anyway costs the player their session.
    let renderer_table_ptr = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            PROFILE_MODEL_REND_TABLE_RVA,
            "PROFILE_MODEL_REND_TABLE_RVA",
        ))
    }
    .unwrap_or(NULL);
    if renderer_table_ptr == NULL {
        let built = unsafe { maybe_build_profile_table_for_loading(base) };
        let after = unsafe {
            safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                PROFILE_MODEL_REND_TABLE_RVA,
                "PROFILE_MODEL_REND_TABLE_RVA",
            ))
        }
        .unwrap_or(NULL);
        if after == NULL {
            append_autoload_debug(format_args!(
                "system-quit-dup: profile-load route abort -- the profile model renderer table at `PROFILE_MODEL_REND_TABLE_RVA` is null and the host could not build one (build_requested={built}). Submitting 05_010_ProfileSelect now would fault in `PROFILE_RENDERER_REFRESH_RVA` reading [null+0x754], which is an access violation, not a refused row"
            ));
            return false;
        }
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route built the profile model renderer table before submitting (was null, now 0x{after:x})"
        ));
    }
    let Ok(wrapper_addr) = game_rva(PROFILE_SELECT_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve ProfileSelect wrapper rva 0x{PROFILE_SELECT_WRAPPER_RVA:x}"
        ));
        return false;
    };
    let Ok(submit_addr) = game_rva(MENU_JOB_SUBMIT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve menu-job submit rva 0x{MENU_JOB_SUBMIT_RVA:x}"
        ));
        return false;
    };
    let job_slot =
        &SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT as *const core::sync::atomic::AtomicUsize as usize;
    SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.store(NULL, Ordering::SeqCst);
    // Latch the profile-load flow as active now (before the ProfileSelect job/Run hook runs) so the
    // native load-confirm MessageBox the own_stepper self-pump triggers is suppressed and cannot
    // crash the game. Cleared on ProfileSelect reset (system_quit_reset_profile_select_state).
    SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE.store(1, Ordering::SeqCst);
    // Safety: the address resolved from a pinned rva on a recognised build, and the three arguments
    // are the slot, the dialog's own owning-window vector and its embedded proxy -- the same triple
    // the game's own title-side callers pass.
    let wrapper: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(wrapper_addr) };
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route FIRE 05_010_ProfileSelect wrapper 0x{wrapper_addr:x}(rcx=job_slot=0x{job_slot:x}, rdx=menu_window_list=dialog+0x50=0x{menu_window_list:x} count={menu_window_list_count}, r8=scene_proxy=0x{scene_proxy:x}) from system_dialog=0x{system_dialog:x}"
    ));
    let ret = unsafe { wrapper(job_slot, menu_window_list, scene_proxy) };
    let job = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    let job_vt = if job >= HEAP_LO {
        unsafe { safe_read_usize(job) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if job < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route 05_010 wrapper returned=0x{ret:x} job_slot=0x{job_slot:x} job=0x{job:x} job_vt=0x{job_vt:x}; no job to submit"
        ));
        return false;
    }
    // Safety: same pinned-rva resolution, with the dialog's own job queue and the slot the wrapper
    // just filled.
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(submit_addr) };
    let submit_queue = system_dialog + 0x10;
    SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.store(menu_window_list, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.store(system_dialog, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(system_dialog, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route SUBMIT job=0x{job:x} job_vt=0x{job_vt:x} via 0x{submit_addr:x}(queue=dialog+0x10=0x{submit_queue:x}, job_slot=0x{job_slot:x}); armed ProfileSelect list observer=0x{menu_window_list:x} -- no slot activation/no load"
    ));
    unsafe { submit(submit_queue, job_slot) };
    let job_after_submit = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route submitted 05_010 wrapper job; job_slot_after=0x{job_after_submit:x}"
    ));
    true
}
