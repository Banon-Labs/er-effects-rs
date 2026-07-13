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
    // Runtime-flavor extension filter: vanilla offers `.sl2`; Seamless offers both `.co2` and
    // `.sl2` so vanilla saves can be loaded/imported while ERSC owns the session. Same mode source
    // as the ingest pipeline (launcher hint, then module latch).
    let seamless = save_picker_seamless_mode_after_settle("system-quit-picker-open");
    let model = if seamless {
        crate::experiments::save_picker::SavePickerModel::open_with_extensions(
            &start_dir,
            &["co2", "sl2"],
        )
    } else {
        crate::experiments::save_picker::SavePickerModel::open(&start_dir, "sl2")
    };
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
        "save-picker: opened in-game picker action=0x{action_obj:x} dir='{}' ext=.{}",
        start_dir.display(),
        crate::experiments::save_picker::active_save_picker_lock()
            .as_ref()
            .map(|model| model.extension().to_owned())
            .unwrap_or_else(|| "<unset>".to_owned())
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
            // IN-GAME (System>Quit) site only: the pick feeds the existing preview/candidate
            // pipeline and reopens the window as the slot view. The STARTUP no-save site does NOT
            // use this native-window path -- it uses the DLL-drawn overlay picker
            // (`save_picker_overlay.rs`) because the game's menu assets are not ready at the
            // held save-check stage.
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

/// Start dir for the STARTUP overlay picker: remembered dir when valid, else the default save
/// root (`%APPDATA%\EldenRing`), else the Wine system drive root. Windows-form paths.
pub(crate) fn save_picker_title_start_dir() -> PathBuf {
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
