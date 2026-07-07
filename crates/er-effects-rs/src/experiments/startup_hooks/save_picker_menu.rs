// In-game save-file picker rendered through the native `05_010_ProfileSelect` window.
//
// Replaces the System>Quit "Load Save Profiles" `GetOpenFileNameW` OS dialog (context switch out
// of the game; user goal 2026-07-07) with the same native 10-row window the character switcher
// already drives. The rows are a browsable directory listing (row 0 = up, rows 1..=8 = dirs +
// mode-locked save files, row 9 = page cycle) staged as synthetic ProfileSummary records; the
// shared model lives in `experiments::save_picker`. Directory/page navigation rebuilds the row
// list in place via the game's own records-changed rebuild (close + menu-pump resubmit as
// fallback). Picking a file feeds the exact validation/preview pipeline the OS picker used
// (`system_quit_ingest_picked_save`) and then reopens the window as the normal slot view, so
// the "pick file -> pick character slot" flow never leaves the game's visual system.

/// 1 while the live `05_010_ProfileSelect` window is OUR file-picker (rows = directory listing).
/// 0 when it is the normal character-slot view.
pub(crate) static SAVE_PICKER_MODE_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// 1 = the picker window was closed for a directory/page change; the menu-pump Run hook must
/// resubmit a fresh `05_010` job (records already restaged) instead of restoring the System UI.
pub(crate) static SAVE_PICKER_REOPEN_PENDING: AtomicUsize = AtomicUsize::new(0);
/// 1 = a file was ingested from the picker; the menu-pump Run hook must resubmit `05_010` as the
/// NORMAL slot view (picker mode already cleared) so the user picks a character slot next.
pub(crate) static SAVE_PICKER_OPEN_SLOTS_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Action object of the "Load Save Profiles" row; `system_quit_open_profile_load_dialog` derives
/// the System dialog (action+0x8), submit queue and window list from it on every (re)submit.
pub(crate) static SAVE_PICKER_ACTION_OBJ: AtomicUsize = AtomicUsize::new(0);
/// Diagnostics / telemetry oracles.
pub(crate) static SAVE_PICKER_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_REPOPULATE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_PICK_REJECT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_RESUBMIT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_STAGED_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Dialog whose row list must be rebuilt in menu-pump ownership (0 = none). Set by a
/// navigation/page activation after restaging records; consumed by the Run hook.
pub(crate) static SAVE_PICKER_REBUILD_PENDING_DIALOG: AtomicUsize = AtomicUsize::new(0);

/// Windows-form (`Z:\...`) string for a possibly Linux-form absolute path; drive-prefixed paths
/// pass through with separators normalized. String twin of `system_quit_path_for_windows`.
fn save_picker_windows_path_string(path: &str) -> String {
    let mut win = if path.starts_with('/') {
        format!("Z:{}", path.replace('/', "\\"))
    } else {
        path.replace('/', "\\")
    };
    while win.ends_with('\\') && win.len() > 3 {
        win.pop();
    }
    win
}

/// Starting directory for the picker: last picked dir (session, then er-effects.toml) when it
/// still exists, else the active save's directory, else the default save root.
fn save_picker_start_dir() -> Option<PathBuf> {
    if let Some(preferred) = crate::config::preferred_save_picker_dir_now() {
        if let Some(text) = preferred.to_str() {
            let windows = PathBuf::from(save_picker_windows_path_string(text));
            if windows.is_dir() {
                return Some(windows);
            }
        }
    }
    if let Ok(dir) = system_quit_env_save_dir() {
        let windows = PathBuf::from(save_picker_windows_path_string(&dir));
        if windows.is_dir() {
            return Some(windows);
        }
    }
    default_save_root()
        .and_then(|root| root.to_str().map(save_picker_windows_path_string))
        .map(PathBuf::from)
        .filter(|root| root.is_dir())
}

/// Stage the model's 10 visible rows as synthetic ProfileSummary records (name field = row
/// label; everything else zeroed). Snapshots the live summary first via the save-swap state, so
/// every existing backout path restores the user's real rows. Menu-thread only (record writes +
/// renderer refresh -- same context the foreign-save preview uses).
unsafe fn save_picker_stage_row_records(
    model: &crate::experiments::save_picker::SavePickerModel,
) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == null {
        append_autoload_debug(format_args!(
            "save-picker: cannot stage rows -- live ProfileSummary unavailable"
        ));
        return false;
    }
    {
        let mut st = system_quit_save_swap_lock();
        if st.summary_snapshot.is_empty() || st.summary_ptr != summary {
            st.summary_ptr = summary;
            st.summary_snapshot = unsafe {
                core::slice::from_raw_parts(summary as *const u8, PROFILE_SUMMARY_TOTAL_BYTES)
                    .to_vec()
            };
        }
        // Mark the summary as replaced so `system_quit_save_swap_restore_profile_summary`
        // restores the user's real rows on any backout path.
        st.preview_applied = true;
    }
    let mut staged = 0usize;
    unsafe {
        for slot in 0..TITLE_PROFILE_SLOT_COUNT {
            let record =
                summary + PROFILE_SUMMARY_RECORD_BASE + slot * PROFILE_SUMMARY_RECORD_STRIDE;
            core::ptr::write_bytes(record as *mut u8, 0, PROFILE_SUMMARY_RECORD_STRIDE);
            PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
            let mut label = model.row_label_utf16(slot);
            if label.is_empty() {
                // The native list builder appends a row ONLY for slots whose
                // `saveSlotsStates[slot]` byte is set (RE-verified: occupancy predicate live
                // 0x140261cd0 reads summary+0x8+slot; bound at dialog+0xb08 = occupied count).
                // Keep ALL 10 slots occupied with a placeholder so row index == slot index ==
                // model row, and cursor math never has to translate sparse row ids.
                label = "-".encode_utf16().collect();
            }
            // Name field is 0x22 bytes (16 UTF-16 units + NUL); the record was zeroed above so
            // truncated copies stay terminated.
            let units = label.len().min(PROFILE_SUMMARY_NAME_BYTES / 2 - 1);
            core::ptr::copy_nonoverlapping(label.as_ptr(), record as *mut u16, units);
            *((summary + PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot) as *mut u8) = 1;
            staged += 1;
        }
    }
    SAVE_PICKER_STAGED_ROW_COUNT.store(staged, Ordering::SeqCst);
    if let Ok(base) = game_module_base() {
        let refresh: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
        unsafe { refresh() };
    }
    append_autoload_debug(format_args!(
        "save-picker: staged {staged} row records dir='{}' page={}/{} entries={}",
        model.current_dir().display(),
        model.page() + 1,
        model.page_count(),
        model.entry_count()
    ));
    true
}

/// Open the in-game file picker from the "Load Save Profiles" row action (menu thread).
/// Mirrors the old OS-picker preflight (restore stale preview, arm the active save snapshot),
/// then stages the browse rows and submits the `05_010_ProfileSelect` window.
pub(crate) unsafe fn system_quit_open_save_picker_menu(action_obj: usize) -> bool {
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker: refused to open -- {reason}"
            ));
            return false;
        }
    };
    unsafe { system_quit_save_swap_restore_profile_summary("save-picker-reopen") };
    if !system_quit_save_swap_arm_original(&save_path) {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let Some(start_dir) = save_picker_start_dir() else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: refused to open -- no readable start directory (preferred/save-dir/default-root all unavailable)"
        ));
        return false;
    };
    // Mode-locked extension: only the container flavor the active runtime loads (user directive
    // 2026-07-06). At this point (in-game menu) the Seamless latch is reliable.
    let extension = crate::telemetry::expected_save_extension();
    let model =
        crate::experiments::save_picker::SavePickerModel::open(&start_dir, extension);
    if !unsafe { save_picker_stage_row_records(&model) } {
        return false;
    }
    *crate::experiments::save_picker::active_save_picker_lock() = Some(model);
    SAVE_PICKER_MODE_ACTIVE.store(1, Ordering::SeqCst);
    SAVE_PICKER_ACTION_OBJ.store(action_obj, Ordering::SeqCst);
    SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
    SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
    let opened = unsafe { system_quit_open_profile_load_dialog(action_obj) };
    if !opened {
        // Roll back: restore rows + drop the model so the System menu stays coherent.
        unsafe { system_quit_save_swap_restore_profile_summary("save-picker-open-failed") };
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
        SAVE_PICKER_ACTION_OBJ.store(0, Ordering::SeqCst);
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker: opened in-game picker action=0x{action_obj:x} dir='{}' ext=.{extension}",
        start_dir.display()
    ));
    true
}

/// Route a `05_010` slot activation while the picker owns the window (menu thread, called from
/// the activate hook BEFORE any character-switch logic). Returns the hook's return value.
pub(crate) unsafe fn save_picker_handle_activation(dialog: usize, cursor: i32) -> usize {
    use crate::experiments::save_picker::PickerActivation;
    if cursor < 0 || cursor as usize >= crate::experiments::save_picker::PICKER_ROW_COUNT {
        return 0;
    }
    let activation = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_mut() else {
            append_autoload_debug(format_args!(
                "save-picker: activation with no model (cursor={cursor}); ignoring"
            ));
            return 0;
        };
        model.activate(cursor as usize)
    };
    match activation {
        PickerActivation::Repopulate => {
            let staged = {
                let guard = crate::experiments::save_picker::active_save_picker_lock();
                match guard.as_ref() {
                    Some(model) => unsafe { save_picker_stage_row_records(model) },
                    None => false,
                }
            };
            if staged {
                SAVE_PICKER_REPOPULATE_COUNT.fetch_add(1, Ordering::SeqCst);
                // Refresh row text via the game's OWN records-changed rebuild (the delete-save
                // flow's primitive): re-reads the rewritten records, rewrites the bound,
                // re-selects the cursor and re-decorates -- no window close, no System-UI flash.
                // The decorate pass reads per-row snapshots, so the record writes above are
                // invisible without it. DEFERRED to the menu-pump Run hook: the native delete
                // flow runs this rebuild as a queued job AFTER the decide returns, never inside
                // the widget's own input dispatch. Fallback there: close + resubmit.
                SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
            }
            0
        }
        PickerActivation::PickedFile(path) => {
            // STARTUP (title) site: no active save to swap -- the pick installs the save
            // redirect and reloads the title. IN-GAME site: the pick feeds the existing
            // preview/candidate pipeline and reopens the window as the slot view.
            if missing_save_selection_pending() {
                if unsafe { save_picker_title_complete_pick(dialog, &path) } {
                    SAVE_PICKER_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
                } else {
                    SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
                }
                return 0;
            }
            let Some(path_str) = path.to_str() else {
                SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
                return 0;
            };
            if unsafe { system_quit_ingest_picked_save(path_str) } {
                SAVE_PICKER_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
                *crate::experiments::save_picker::active_save_picker_lock() = None;
                SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
                SAVE_PICKER_OPEN_SLOTS_PENDING.store(1, Ordering::SeqCst);
                unsafe { save_picker_native_close(dialog, "picked-file") };
            } else {
                // Invalid container: stay in the picker so the user can choose another file.
                // The ingest pipeline already restaged nothing (preview only applies on
                // success), but our browse rows were untouched -- the window stays coherent.
                SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
            }
            0
        }
        PickerActivation::Ignored => 0,
    }
}

/// Native cancel-close (SetResult(Failed) + window close) -- same primitive the character-switch
/// pick uses; runs in menu ownership from the activate hook.
unsafe fn save_picker_native_close(dialog: usize, reason: &str) {
    if let Ok(close_addr) = game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) {
        let close_fn: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(close_addr) };
        unsafe { close_fn(dialog) };
        SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: native-closed picker window dialog=0x{dialog:x} reason={reason}"
        ));
    } else {
        append_autoload_debug(format_args!(
            "save-picker: FAILED to resolve native close rva for dialog=0x{dialog:x} reason={reason}"
        ));
    }
}

/// True while a picker-driven close must NOT run the normal restore path (a resubmit is queued).
pub(crate) fn save_picker_resubmit_pending() -> bool {
    SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0
        || SAVE_PICKER_OPEN_SLOTS_PENDING.load(Ordering::SeqCst) != 0
}

/// Menu-pump-owned in-place list rebuild (called from the MenuWindowJob::Run hook). Runs the
/// native records-changed rebuild queued by a picker navigation; falls back to close+resubmit
/// when the rebuild fn cannot be resolved.
pub(crate) unsafe fn save_picker_menu_pump_rebuild() {
    let dialog = SAVE_PICKER_REBUILD_PENDING_DIALOG.swap(0, Ordering::SeqCst);
    if dialog == 0 || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 {
        return;
    }
    if let Ok(rebuild_addr) = game_rva(PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA) {
        let rebuild: unsafe extern "system" fn(usize) =
            unsafe { std::mem::transmute(rebuild_addr) };
        unsafe { rebuild(dialog) };
        append_autoload_debug(format_args!(
            "save-picker: menu-pump in-place list rebuild dialog=0x{dialog:x} via 0x{rebuild_addr:x}"
        ));
    } else {
        SAVE_PICKER_REOPEN_PENDING.store(1, Ordering::SeqCst);
        unsafe { save_picker_native_close(dialog, "repopulate-no-rebuild-rva") };
    }
}

/// Menu-pump-owned resubmit: called from `system_quit_menu_window_job_run_hook` (the proven
/// submit context) once the closed picker window has left the list. Returns true when a resubmit
/// was performed (or is still pending), i.e. the caller must skip the System-UI restore.
pub(crate) unsafe fn save_picker_menu_pump_resubmit() -> bool {
    if !save_picker_resubmit_pending() {
        return false;
    }
    if SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0 {
        // Old window still live; wait for its close to finish.
        return true;
    }
    let action_obj = SAVE_PICKER_ACTION_OBJ.load(Ordering::SeqCst);
    if action_obj == 0 {
        append_autoload_debug(format_args!(
            "save-picker: resubmit pending but action object lost; abandoning reopen"
        ));
        SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
        SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
        return false;
    }
    let reopen_as_picker = SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0;
    SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
    SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
    let opened = unsafe { system_quit_open_profile_load_dialog(action_obj) };
    if opened {
        SAVE_PICKER_RESUBMIT_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: menu-pump resubmitted 05_010 window as {} (action=0x{action_obj:x})",
            if reopen_as_picker { "picker page" } else { "slot view" }
        ));
        return true;
    }
    append_autoload_debug(format_args!(
        "save-picker: menu-pump resubmit FAILED (action=0x{action_obj:x}); falling back to System-UI restore"
    ));
    if reopen_as_picker {
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
    }
    false
}

/// Clear picker state on any full reset of the ProfileSelect hide machinery (backout/restore).
pub(crate) fn save_picker_reset(source: &str) {
    if missing_save_selection_pending() {
        // STARTUP (title) picker: the model and the staged browse rows outlive any single window.
        // Backing out of the dialog returns to the no-save title menu with the rows still staged,
        // so the native Load Game row re-opens the SAME picker (and the SetState deny keeps every
        // world-entry path closed). State only clears when a save is picked.
        append_autoload_debug(format_args!(
            "save-picker: reset skipped while missing-save selection pending (source={source}); picker stays armed for native Load Game reopen"
        ));
        return;
    }
    let was_active = SAVE_PICKER_MODE_ACTIVE.swap(0, Ordering::SeqCst) != 0;
    let had_model = crate::experiments::save_picker::active_save_picker_lock()
        .take()
        .is_some();
    SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
    SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
    SAVE_PICKER_ACTION_OBJ.store(0, Ordering::SeqCst);
    if was_active || had_model {
        SAVE_PICKER_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: reset (source={source}, was_active={was_active}, had_model={had_model})"
        ));
    }
}

// ===========================================================================
// STARTUP (TITLE) MISSING-SAVE PICKER
// ===========================================================================
//
// When the DLL attaches with no configured save and no readable default, the title boots to its
// NATIVE no-save menu (the save-data job passes through and completes empty; the SetState detour
// denies only world-entry states 4/5). Once the title main menu is interactive, this flow stages
// the browse rows into the (empty, boot-allocated) ProfileSummary and fires the native Load Game
// row -- the title's own 05_010 ProfileLoadDialog opens showing the file browser. Selection is
// routed by the SAME activate hook as the in-game picker; picking a valid save installs the
// save redirect (complete_missing_save_selection_from_picker), restores the summary, and fires
// the native return-to-title reload so the game re-reads the now-redirected save.

/// One-shot: the title picker auto-opened once this session (cancel leaves the rows staged, so
/// the native Load Game row is the reopen path -- no repeated auto-open fighting the user).
pub(crate) static SAVE_PICKER_TITLE_AUTO_OPENED: AtomicUsize = AtomicUsize::new(0);
/// Telemetry: title picker auto-open fired / pick completed / reload fired.
pub(crate) static SAVE_PICKER_TITLE_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_TITLE_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_TITLE_RELOAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Session singleton STEP_BeginLogo hard-asserts at entry (abs 0x144588e98 = base+0x4588e98,
/// checked at 0x140b0c2c3); only SetState(2) when it is non-null.
const TITLE_SESSION_SINGLETON_RVA: usize = 0x4588e98;

/// Start dir for the STARTUP picker: remembered dir when valid, else the default save root
/// (`%APPDATA%\EldenRing`), else the Wine system drive root.
fn save_picker_title_start_dir() -> PathBuf {
    if let Some(preferred) = crate::config::preferred_save_picker_dir_now() {
        if let Some(text) = preferred.to_str() {
            let windows = PathBuf::from(save_picker_windows_path_string(text));
            if windows.is_dir() {
                return windows;
            }
        }
    }
    if let Some(root) = default_save_root()
        && let Some(text) = root.to_str()
    {
        let windows = PathBuf::from(save_picker_windows_path_string(text));
        if windows.is_dir() {
            return windows;
        }
    }
    PathBuf::from("Z:\\")
}

/// Menu-pump tick for the startup picker (called from the MenuWindowJob::Run hook, i.e. in menu
/// ownership). Auto-opens the picker once the native no-save title menu is interactive.
pub(crate) unsafe fn save_picker_title_pump_tick(base: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if !missing_save_selection_pending()
        || SAVE_PICKER_TITLE_AUTO_OPENED.load(Ordering::SeqCst) != 0
    {
        return;
    }
    let owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
    if owner < HEAP_LO || owner == NULL {
        return;
    }
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(NULL);
    if dialog < HEAP_LO || dialog == NULL {
        return;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    // Menu opened (press-any happened, registrar fired) and its flow-chain queue has drained
    // (main-menu rows dispatch input only while dialog+0x10 holds no job -- RE 2026-07-07).
    let menu_opened = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & 0xff)
        .unwrap_or(0);
    if menu_opened == 0 {
        return;
    }
    let Ok(ready_addr) = game_rva(MENU_JOB_QUEUE_READY_RVA) else {
        return;
    };
    let ready: unsafe extern "system" fn(usize) -> u8 =
        unsafe { std::mem::transmute(ready_addr) };
    if unsafe { ready(dialog + 0x10) } == 0 {
        return;
    }
    // Interactive no-save title menu reached: stage the browser rows and fire the native
    // Load Game row exactly once.
    let Some(action) = (unsafe { title_menu_action_ready(owner, base) }) else {
        return;
    };
    let start_dir = save_picker_title_start_dir();
    let extension = crate::telemetry::expected_save_extension();
    let model = crate::experiments::save_picker::SavePickerModel::open(&start_dir, extension);
    if !unsafe { save_picker_stage_row_records(&model) } {
        return;
    }
    *crate::experiments::save_picker::active_save_picker_lock() = Some(model);
    SAVE_PICKER_MODE_ACTIVE.store(1, Ordering::SeqCst);
    SAVE_PICKER_TITLE_AUTO_OPENED.store(1, Ordering::SeqCst);
    SAVE_PICKER_TITLE_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    let run: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + MENU_MEMBER_FUNC_JOB_RUN_RVA) };
    append_autoload_debug(format_args!(
        "save-picker: TITLE auto-open -- firing native Load Game node 0x{:x}(node=0x{:x}) dir='{}' ext=.{extension}",
        base + MENU_MEMBER_FUNC_JOB_RUN_RVA,
        action.node,
        start_dir.display()
    ));
    unsafe { run(action.node) };
}

/// Title-picker pick completion (menu thread, from the activate hook): install the redirect,
/// restore the real (empty) summary, close the dialog, and fire the native return-to-title
/// reload so the game re-reads the save through the redirect.
unsafe fn save_picker_title_complete_pick(dialog: usize, path: &Path) -> bool {
    if !crate::experiments::complete_missing_save_selection_from_picker(path) {
        return false;
    }
    SAVE_PICKER_TITLE_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
    // Restore the boot summary (all-empty snapshot) BEFORE the reload: the reload's save-data
    // job repopulates it from the redirected save; leaving browse rows staged across the reload
    // would race the native re-read.
    unsafe { system_quit_save_swap_restore_profile_summary("title-picker-picked") };
    *crate::experiments::save_picker::active_save_picker_lock() = None;
    SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
    unsafe { save_picker_native_close(dialog, "title-picked-file") };
    let owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
    let orig = TITLE_SETSTATE_TRACE_ORIG.load(Ordering::SeqCst);
    let session = game_module_base()
        .ok()
        .and_then(|base| unsafe { safe_read_usize(base + TITLE_SESSION_SINGLETON_RVA) })
        .unwrap_or(0);
    if owner > 0x10000 && orig != 0 && orig != HOOK_ORIGINAL_UNSET && session != 0 {
        // Native return-to-title: the game's own SetState(2) (STEP_BeginLogo; with splash-skip
        // applied it falls through to BeginTitle) replays title build + menu-open + the
        // save-data read -- now through the redirect, so Continue/Load Game come back real.
        let set_state: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(orig) };
        unsafe { set_state(owner, TITLE_STEP_BEGIN_LOGO) };
        SAVE_PICKER_TITLE_RELOAD_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: TITLE pick complete -- fired native SetState(owner=0x{owner:x}, {TITLE_STEP_BEGIN_LOGO}) title reload; save re-read rides the redirect"
        ));
    } else {
        append_autoload_debug(format_args!(
            "save-picker: TITLE pick complete but reload NOT fired (owner=0x{owner:x} orig=0x{orig:x} session=0x{session:x}); redirect is active -- native menu still needs a manual title return"
        ));
    }
    true
}
