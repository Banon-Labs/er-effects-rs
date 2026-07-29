// In-game save-file picker rendered through the native `05_010_ProfileSelect` window.
//
// Replaces the System>Quit "Load Save Profiles" `GetOpenFileNameW` OS dialog (context switch out
// of the game) with the same native 10-row window the character switcher already drives. The rows
// are a browsable directory listing -- up, drive cycler, `[ new ]` in destination intent, dirs +
// mode-locked save files, page cycler -- staged as synthetic ProfileSummary records; the shared
// model lives in `experiments::save_picker` and owns the row layout (see its module docs for the
// order and why nothing sits at a fixed index). Directory/drive/page navigation rebuilds the row
// list in place via the game's own records-changed rebuild (close + menu-pump resubmit as
// fallback). Picking a file feeds the exact validation/preview pipeline the OS picker used
// (`system_quit_ingest_picked_save`) and then reopens the window as the normal slot view, so
// the "pick file -> pick character slot" flow never leaves the game's visual system.
//
// The only input this window gives the DLL is ROW ACTIVATION: `system_quit_profile_load_activate_hook`
// intercepts `CS::ProfileLoadDialog` vtable slot 20 (`0x9a4670`) and reads the highlighted list
// index out of `dialog+0xb0c`. Cursor movement, back and every other press stay inside the game's
// own list widget. So every browse action -- including switching drives -- has to BE a row.

/// 1 while the live `05_010_ProfileSelect` window is OUR file-picker (rows = directory listing).
/// 0 when it is the normal character-slot view.
pub(crate) use er_telemetry::counters::SAVE_PICKER_MODE_ACTIVE;
/// 1 = the picker window was closed for a directory/page change; the menu-pump Run hook must
/// resubmit a fresh `05_010` job (records already restaged) instead of restoring the System UI.
pub(crate) use er_telemetry::counters::SAVE_PICKER_REOPEN_PENDING;
/// 1 = a file was ingested from the picker; the menu-pump Run hook must resubmit `05_010` as the
/// NORMAL slot view (picker mode already cleared) so the user picks a character slot next.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_SLOTS_PENDING;
/// Action object of the "Load Save Profiles" row; `system_quit_open_profile_load_dialog` derives
/// the System dialog (action+0x8), submit queue and window list from it on every (re)submit.
pub(crate) use er_telemetry::counters::SAVE_PICKER_ACTION_OBJ;
/// 1 while the live picker is the save-DESTINATION chooser (save-game-flow WP3) instead of the
/// load-source browser: row 1 is a pinned `[ new ]`, and activation feeds the save flow.
pub(crate) use er_telemetry::counters::SAVE_PICKER_DEST_MODE;
/// System/Quit dialog the live picker window was submitted from; the menu-pump resubmit reopens
/// through it (the destination picker is opened by the save flow, which has no row action object).
pub(crate) use er_telemetry::counters::SAVE_PICKER_SYSTEM_DIALOG;
/// Diagnostics / telemetry oracles.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_REPOPULATE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_REJECT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_RESUBMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_CANCEL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_STAGED_ROW_COUNT;
/// Dialog whose row list must be rebuilt in menu-pump ownership (0 = none). Set by a
/// navigation/page activation after restaging records; consumed by the Run hook.
pub(crate) use er_telemetry::counters::SAVE_PICKER_REBUILD_PENDING_DIALOG;

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

/// Write the model's visible browse rows into the live ProfileSummary records (record zeroed, name
/// field = row label, slot marked occupied) and mark every slot BEYOND them unoccupied. Pure record
/// transport -- no snapshot bookkeeping and no renderer refresh -- shared by the staging path below
/// and the list-builder re-stage hook. Returns the number of OCCUPIED (visible) rows.
///
/// Occupancy is the row's existence, not a decoration. The native list builder `FUN_140875590`
/// (1.16.2) walks slots 0..10 and appends a row only when the occupancy predicate `FUN_140261cd0`
/// -- literally `ProfileSummary::saveSlotsStates[slot]`, summary+0x8+slot -- returns true, taking
/// the row's name/level/playtime from the record at summary+0x18+slot*0x2a0. Two consequences:
///
///   * a slot marked unoccupied produces NO row at all. That is the only way to make a short
///     listing render nothing below the last entry: a zeroed record still renders as a name plus
///     `Level 0` and `0:00:00`, because those fields exist and are simply zero.
///   * the appended rows are COMPACTED in slot order, so `slot index == visible list index` holds
///     only while the occupied slots are a contiguous PREFIX. They are: the model's visible rows
///     are dense by construction (`visible_row_count`), so the list index the activation hook reads
///     from `dialog+0xb0c`, the slot the row-populate hook reads back from `rowModel+0x8`, and the
///     model row are all the same number.
unsafe fn save_picker_write_row_records(
    model: &crate::experiments::save_picker::SavePickerModel,
    summary: usize,
) -> usize {
    let visible = model
        .visible_row_count()
        .min(TITLE_PROFILE_SLOT_COUNT)
        .min(crate::experiments::save_picker::PICKER_ROW_COUNT);
    unsafe {
        for slot in 0..TITLE_PROFILE_SLOT_COUNT {
            let record =
                summary + PROFILE_SUMMARY_RECORD_BASE + slot * PROFILE_SUMMARY_RECORD_STRIDE;
            core::ptr::write_bytes(record as *mut u8, 0, PROFILE_SUMMARY_RECORD_STRIDE);
            PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
            if slot >= visible {
                // Beyond the listing: no row. The record is already zeroed above, so nothing stale
                // survives if the game's own save path (`FUN_140262270`, which also runs
                // `MarkProfileIndexAsUsed`) flips this slot back to occupied between here and the
                // native build -- and the list-builder re-stage hook re-runs this whole pass at
                // every build site precisely to close that window.
                *((summary + PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot) as *mut u8) = 0;
                continue;
            }
            let mut label = model.row_label_utf16(slot);
            if label.is_empty() {
                // Unreachable: every visible row's label is non-empty by construction (pinned by
                // the model's `every_visible_row_has_a_non_empty_label` test). Kept because an
                // occupied slot with an empty name would fail the empty-slot activation guard, and
                // dropping the row instead would break the prefix and misalign every row below it.
                label = "-".encode_utf16().collect();
            }
            // Name field is 0x22 bytes (16 UTF-16 units + NUL); the record was zeroed above so
            // truncated copies stay terminated.
            let units = label.len().min(PROFILE_SUMMARY_NAME_BYTES / 2 - 1);
            core::ptr::copy_nonoverlapping(label.as_ptr(), record as *mut u16, units);
            *((summary + PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot) as *mut u8) = 1;
        }
    }
    visible
}

/// Stage the model's visible rows as synthetic ProfileSummary records (name field = row label;
/// everything else zeroed) and leave the slots beyond them unoccupied. Snapshots the live summary
/// first via the save-swap state -- occupancy bytes included -- so every existing backout path
/// restores the user's real rows. Menu-thread only (record writes + renderer refresh -- same
/// context the foreign-save preview uses).
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
    let staged = unsafe { save_picker_write_row_records(model, summary) };
    SAVE_PICKER_STAGED_ROW_COUNT.store(staged, Ordering::SeqCst);
    if let Ok(base) = game_module_base() {
        let refresh: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
        unsafe { refresh() };
    }
    append_autoload_debug(format_args!(
        "save-picker: staged {staged} occupied row records ({} slots left unoccupied) dir='{}' page={}/{} entries={} drives={}",
        TITLE_PROFILE_SLOT_COUNT.saturating_sub(staged),
        model.current_dir().display(),
        model.page() + 1,
        model.page_count(),
        model.entry_count(),
        model.drive_count()
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
    SAVE_PICKER_SYSTEM_DIALOG.store(
        unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(0),
        Ordering::SeqCst,
    );
    SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
    SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
    let opened = unsafe { system_quit_open_profile_load_dialog(action_obj) };
    if !opened {
        // Roll back: restore rows + drop the model so the System menu stays coherent.
        unsafe { system_quit_save_swap_restore_profile_summary("save-picker-open-failed") };
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
        SAVE_PICKER_ACTION_OBJ.store(0, Ordering::SeqCst);
        SAVE_PICKER_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
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

/// Open the `05_010` picker as the save-DESTINATION chooser for the Save Game flow (save-game-flow
/// WP3). Menu-pump owned: called from `system_quit_menu_window_run_post` after the tick stages
/// `SAVE_DEST_OPEN_PICKER_PENDING`, i.e. the same submit context the load picker's resubmit uses.
///
/// Differences from the load-source picker, all deliberate:
///   * start dir = the LOADED save's own directory, not the remembered preferred dir -- "save
///     next to the save you loaded" is the expected default and the remembered dir belongs to the
///     load flow;
///   * NO save-swap byte preview is armed: nothing foreign is previewed here, and the safety
///     snapshot of the live save is taken later, at the fire gate, by `save_dest_arm_redirect`;
///   * the model carries the loaded save's filename so the `[ new ]` row writes that leaf.
pub(crate) unsafe fn system_quit_open_save_dest_picker(system_dialog: usize) -> bool {
    const HEAP_LO: usize = 0x10000;
    if system_dialog < HEAP_LO || system_dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-dest-picker: refused to open -- System dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            append_autoload_debug(format_args!(
                "save-dest-picker: refused to open -- {reason}"
            ));
            return false;
        }
    };
    let loaded_file_name = match Path::new(&save_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some(name) => name.to_owned(),
        None => {
            append_autoload_debug(format_args!(
                "save-dest-picker: refused to open -- loaded save '{save_path}' has no file name"
            ));
            return false;
        }
    };
    // Start where the loaded save lives; fall back to the default save root only if that directory
    // is gone.
    let start_dir = system_quit_env_save_dir()
        .ok()
        .map(|dir| PathBuf::from(save_picker_windows_path_string(&dir)))
        .filter(|dir| dir.is_dir())
        .or_else(|| {
            default_save_root()
                .and_then(|root| root.to_str().map(save_picker_windows_path_string))
                .map(PathBuf::from)
                .filter(|root| root.is_dir())
        });
    let Some(start_dir) = start_dir else {
        append_autoload_debug(format_args!(
            "save-dest-picker: refused to open -- neither the loaded save's directory nor the default save root is readable"
        ));
        return false;
    };
    unsafe { system_quit_save_swap_restore_profile_summary("save-dest-picker-open") };
    // Same mode-locked filter as the load picker: the destination list shows the containers the
    // active runtime flavor understands.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-save-dest-picker-open");
    let extensions: &[&str] = if seamless {
        &["co2", "sl2"]
    } else {
        &["sl2"]
    };
    let model = crate::experiments::save_picker::SavePickerModel::open_destination(
        &start_dir,
        extensions,
        &loaded_file_name,
    );
    if !unsafe { save_picker_stage_row_records(&model) } {
        return false;
    }
    *crate::experiments::save_picker::active_save_picker_lock() = Some(model);
    SAVE_PICKER_MODE_ACTIVE.store(1, Ordering::SeqCst);
    SAVE_PICKER_DEST_MODE.store(1, Ordering::SeqCst);
    SAVE_PICKER_ACTION_OBJ.store(0, Ordering::SeqCst);
    SAVE_PICKER_SYSTEM_DIALOG.store(system_dialog, Ordering::SeqCst);
    SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
    SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
    if !unsafe { system_quit_open_profile_load_dialog_on(system_dialog) } {
        unsafe { system_quit_save_swap_restore_profile_summary("save-dest-picker-open-failed") };
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
        SAVE_PICKER_DEST_MODE.store(0, Ordering::SeqCst);
        SAVE_PICKER_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-dest-picker: 05_010 submit FAILED for dialog=0x{system_dialog:x}"
        ));
        return false;
    }
    SAVE_DEST_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-dest-picker: opened destination browser dialog=0x{system_dialog:x} dir='{}' new_file='{loaded_file_name}' seamless={seamless}",
        start_dir.display()
    ));
    true
}

/// Handle a destination-browser activation (menu thread, from `save_picker_handle_activation`).
/// `target` already exists -> the Box3 overwrite confirm; otherwise the commit is staged and the
/// picker closes so the save-flow tick can close the menus and fire.
unsafe fn save_dest_handle_picked_target(dialog: usize, target: PathBuf, from_new_row: bool) {
    let exists = target.is_file();
    if exists {
        SAVE_DEST_TARGET_EXISTING_COUNT.fetch_add(1, Ordering::SeqCst);
        save_dest_set_target(target, if from_new_row { "new-row-existing" } else { "picked-file" });
        // Box3 is hosted by the PICKER dialog (the game raises its own confirms over 05_010 the
        // same way), so it does not contend with the System dialog queue that owns the picker
        // window job. Submitted inline here (menu thread); a not-ready queue leaves the pending
        // latch for the next menu pump.
        save_flow_box_set_host_dialog(dialog);
        SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_OVERWRITE_FILE, Ordering::SeqCst);
        SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
        SAVE_FLOW_STAGE.store(SAVE_FLOW_STAGE_BOX3_WAIT, Ordering::SeqCst);
        if unsafe { save_flow_submit_box(SAVE_FLOW_BOX_OVERWRITE_FILE) } {
            SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
        }
        return;
    }
    SAVE_DEST_TARGET_NEW_COUNT.fetch_add(1, Ordering::SeqCst);
    save_dest_set_target(target, "new-row");
    save_dest_stage_commit_and_close_picker(dialog, "new-file");
}

/// Stage the destination commit and close the browser. The save-flow tick takes over once the
/// picker window has finished tearing down (the native close also restores the user's real
/// ProfileSummary rows and re-shows the System windows, which is exactly the state the close-all
/// sequence expects).
unsafe fn save_dest_stage_commit_and_close_picker(dialog: usize, reason: &str) {
    SAVE_DEST_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_DEST_COMMIT_PENDING.store(1, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    SAVE_FLOW_STAGE.store(SAVE_FLOW_STAGE_DEST_BROWSE, Ordering::SeqCst);
    save_flow_box_clear();
    unsafe { save_picker_native_close(dialog, reason) };
    append_autoload_debug(format_args!(
        "save-dest: commit staged (reason={reason}) target='{}'; picker closing, the save-flow tick will close the menus and fire",
        save_dest_target()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unset>".to_owned())
    ));
}

/// Route a `05_010` slot activation while the picker owns the window (menu thread, called from
/// the activate hook BEFORE any character-switch logic). Returns the hook's return value.
///
/// This is the ONLY signal the native window hands us, so it carries every browse action: up,
/// enter directory, switch drive, page, pick file, `[ new ]`. The model decides which from the row
/// index; a listing change of any kind comes back as `Repopulate` and is serviced identically.
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
        PickerActivation::PickedFile(path)
            if SAVE_PICKER_DEST_MODE.load(Ordering::SeqCst) != 0 =>
        {
            // DESTINATION browser: an existing container was picked as the save target, so the
            // final overwrite confirm decides. No ingest/preview -- nothing is being loaded.
            unsafe { save_dest_handle_picked_target(dialog, path, false) };
            0
        }
        PickerActivation::PickedNewFile(path) => {
            // `[ new ]`: save into the browsed folder under the loaded save's own filename. If
            // that file already exists there, fall into the Box3 overwrite confirm rather than
            // silently clobbering it.
            unsafe { save_dest_handle_picked_target(dialog, path, true) };
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
    // Reopen through the System dialog the window was submitted from. The destination browser has
    // no row action object at all (the save flow opens it), so the dialog -- not the action -- is
    // the identity that survives both paths.
    let system_dialog = SAVE_PICKER_SYSTEM_DIALOG.load(Ordering::SeqCst);
    if system_dialog == 0 {
        append_autoload_debug(format_args!(
            "save-picker: resubmit pending but the owning System dialog was lost; abandoning reopen"
        ));
        SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
        SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
        return false;
    }
    let reopen_as_picker = SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0;
    SAVE_PICKER_REOPEN_PENDING.store(0, Ordering::SeqCst);
    SAVE_PICKER_OPEN_SLOTS_PENDING.store(0, Ordering::SeqCst);
    let opened = unsafe { system_quit_open_profile_load_dialog_on(system_dialog) };
    if opened {
        SAVE_PICKER_RESUBMIT_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: menu-pump resubmitted 05_010 window as {} (dialog=0x{system_dialog:x})",
            if reopen_as_picker { "picker page" } else { "slot view" }
        ));
        return true;
    }
    append_autoload_debug(format_args!(
        "save-picker: menu-pump resubmit FAILED (dialog=0x{system_dialog:x}); falling back to System-UI restore"
    ));
    if reopen_as_picker {
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
    }
    false
}

/// Escape text for the Scaleform-HTML SetText path (the `ErStats` row fields parse with bHTML=1,
/// so a character/file name containing `&`, `<` or `>` must not be interpreted as markup).
fn save_picker_html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// One dim Scaleform-HTML line for the browse rows' `ErStats` fields (same size/color language as
/// the stats panel's attribute lines), NUL-terminated UTF-16 for the native SetText wrapper. An
/// empty `text` yields a bare NUL so the field renders blank.
fn save_picker_browse_html_utf16(text: &str) -> Vec<u16> {
    // Matches the stats panel's font size so browse info and attribute lines read identically.
    const SIZE: &str = "19";
    // The stats panel's dim label color -- browse info is secondary to the row name.
    const COLOR: &str = "#8f887a";
    if text.is_empty() {
        return vec![0];
    }
    let mut s = String::from("<p align=\"left\"><font size=\"");
    s.push_str(SIZE);
    s.push_str("\" color=\"");
    s.push_str(COLOR);
    s.push_str("\">");
    s.push_str(&save_picker_html_escape(text));
    s.push_str("</font></p>");
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Character budget for the per-file character list line (the four-attribute stats line occupies
/// roughly this width at the same font size, so the list clips no earlier than the stats did).
const SAVE_PICKER_BROWSE_LINE_CHAR_BUDGET: usize = 44;

/// The two `ErStats` lines for ProfileSelect row `row` while the browse picker owns the window.
/// File rows show the file's REAL character info: active-slot count on the top line and the
/// characters' names + levels on the bottom line (as many as fit the budget, then a `+k` overflow
/// marker). Every other row (up/drive, directory, page cycle, placeholder) gets EMPTY lines so
/// neither leftover row text nor per-slot attribute stats render as junk there. `None` when the
/// picker does not own the rows (the normal character-slot view keeps the attribute stats panel).
/// Generated text uses `/` separators and never inserts commas (comma-safe labels,
/// er-effects-rs-dly6); names pass through with HTML escaping only.
pub(crate) fn save_picker_browse_stats_lines(row: usize) -> Option<(Vec<u16>, Vec<u16>)> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let model = guard.as_ref()?;
    let Some(chars) = model.row_file_characters(row) else {
        // Non-file row: blank both lines.
        return Some((vec![0], vec![0]));
    };
    let top = if chars.len() == 1 {
        "1 CHARACTER".to_owned()
    } else {
        format!("{} CHARACTERS", chars.len())
    };
    let mut bottom = String::new();
    let mut shown = 0usize;
    for info in chars {
        let seg = format!("{} LV {}", info.name, info.level);
        let sep = if bottom.is_empty() { "" } else { " / " };
        if !bottom.is_empty()
            && bottom.chars().count() + sep.chars().count() + seg.chars().count()
                > SAVE_PICKER_BROWSE_LINE_CHAR_BUDGET
        {
            break;
        }
        bottom.push_str(sep);
        bottom.push_str(&seg);
        shown += 1;
    }
    if shown < chars.len() {
        bottom.push_str(&format!(" +{}", chars.len() - shown));
    }
    Some((
        save_picker_browse_html_utf16(&top),
        save_picker_browse_html_utf16(&bottom),
    ))
}

/// Whether ProfileSelect row `row` keeps the native per-slot info fields (`Level` caption, `Level`
/// value, `PlayTime`) while the browse picker owns the window -- true only for save-FILE rows.
///
/// `None` when the picker does NOT own the rows. That is the load-bearing half of the scope: the
/// vanilla character-slot views, the title-screen Load Game list first among them, render from the
/// game's own records and must be left exactly as the game draws them. Same ownership gate as
/// [`save_picker_browse_stats_lines`], so the two cannot disagree about who owns a row.
pub(crate) fn save_picker_row_shows_native_slot_info(row: usize) -> Option<bool> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    Some(guard.as_ref()?.row_shows_native_slot_info(row))
}

#[cfg(test)]
mod save_picker_row_slot_info_tests {
    use super::*;

    /// SCOPE PROOF for the non-file-row Level/PlayTime suppression: with no picker owning the rows
    /// -- the state the vanilla character-slot views run in, the title-screen Load Game list among
    /// them -- the gate answers `None` for every row, and `None` is the only answer the populate
    /// hook treats as "leave this row exactly as the game drew it". A regression that made the
    /// suppression global would have to make this return `Some` here first.
    #[test]
    fn no_picker_means_no_row_is_ever_classified() {
        assert_eq!(
            SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst),
            0,
            "no picker session may be active in a unit test"
        );
        for row in 0..crate::experiments::save_picker::PICKER_ROW_COUNT {
            assert_eq!(
                save_picker_row_shows_native_slot_info(row),
                None,
                "row {row} was classified without a picker owning the rows"
            );
        }
    }
}

/// Entry hook on the native ProfileSelect item-list builder (`PROFILE_SELECT_LIST_BUILDER_RVA`,
/// FUN_140875590): while the browse picker owns the `05_010` rows, RE-STAGE the browse-row records
/// immediately before the native builder turns ProfileSummary records into visible list rows.
///
/// Root cause of the stray current-character row (er-effects-rs-xlqh): the ProfileSummary records
/// are GAME-OWNED and volatile in-world. Every save the game performs runs the save-write path
/// `FUN_14067b940`, which calls `CS::ProfileSummary::MarkProfileIndexAsUsed(summary, saveSlot)`
/// and then `FUN_140262270(summary, saveSlot)` -- and `FUN_140262270` rewrites the ACTIVE slot's
/// record from the LIVE `mainPlayerGameData` (`wcsncpy(record.name, pgd.name, 0x10)` + level +
/// playtime + rune memory + map + face data; static RE, 1.16.2 dump). A save landing between our
/// row staging and the builder's record read left that slot's record holding the LOADED character,
/// which then rendered as a stray browse row (user report: `[ up .. ]`, <current character name>,
/// <save file name>). Rewriting the records here, on the same menu thread that immediately reads
/// them, closes that window for EVERY build site with one seam -- the dialog ctor/bind paths and
/// the delete-flow in-place rebuild (`PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA`) all call this builder.
pub(crate) unsafe extern "system" fn save_picker_profile_list_builder_hook(
    out_list: usize,
) -> usize {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0 || missing_save_selection_pending() {
        let summary = unsafe { system_quit_profile_summary_ptr() };
        if summary != TITLE_OWNER_SCAN_START_ADDRESS {
            let staged = {
                let guard = crate::experiments::save_picker::active_save_picker_lock();
                guard
                    .as_ref()
                    .map(|model| unsafe { save_picker_write_row_records(model, summary) })
            };
            if let Some(staged) = staged {
                let n = SAVE_PICKER_LIST_BUILDER_RESTAGE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= 8 || n.is_power_of_two() {
                    append_autoload_debug(format_args!(
                        "save-picker: re-staged {staged} browse rows at native list build #{n} (game-save record-stomp guard)"
                    ));
                }
            }
        }
    }
    let orig = SAVE_PICKER_LIST_BUILDER_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        // Unreachable in practice (the trampoline is stored before the hook is enabled); mirror
        // the native return (the out-list pointer) rather than crash.
        return out_list;
    }
    let f: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(out_list) }
}

/// Install the list-builder re-stage hook (idempotent; mirrors the row-populate install idiom).
pub(crate) fn install_save_picker_list_builder_hook() {
    if SAVE_PICKER_LIST_BUILDER_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "save-picker: list-builder MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(PROFILE_SELECT_LIST_BUILDER_RVA) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve list-builder rva 0x{PROFILE_SELECT_LIST_BUILDER_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_profile_list_builder_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_LIST_BUILDER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable list-builder failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SAVE_PICKER_LIST_BUILDER_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked ProfileSelect list builder FUN_140875590 0x{addr:x}; browse rows re-stage at every native list build"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: list-builder MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new list-builder failed: {status:?}"
        )),
    }
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
    SAVE_PICKER_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
    // The DEST-mode latch dies with the window, but the chosen destination and the commit latch
    // deliberately do NOT: closing the picker is exactly how a confirmed destination commit
    // proceeds, and the save-flow tick still needs the target after this reset runs.
    let was_destination = SAVE_PICKER_DEST_MODE.swap(0, Ordering::SeqCst) != 0;
    if was_active || had_model {
        SAVE_PICKER_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: reset (source={source}, was_active={was_active}, had_model={had_model}, destination={was_destination})"
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
