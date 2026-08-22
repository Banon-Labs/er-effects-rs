use super::*;

// In-game save-file picker rendered through the native `05_010_ProfileSelect` window.
//
// Replaces the System>Quit "Load Character from File" `GetOpenFileNameW` OS dialog (context switch
// out of the game) with the same native 10-row window the character switcher already drives. The
// rows are a browsable directory listing -- the drive switcher ALWAYS FIRST when present, then
// destination-only `[ new ]`, up, dirs + mode-locked save files -- staged as synthetic
// ProfileSummary records; the shared model lives in `experiments::save_picker` and owns the row
// layout (see its module docs for the order and derived indices). It is also the surface the Save Game row
// press opens directly, with no confirm in front of it. Directory/drive navigation and edge-hover
// scroll-window restaging rebuild the row list in place via the game's own records-changed rebuild
// (close + menu-pump resubmit as fallback). Picking a file feeds the validation/preview pipeline
// the OS picker used (`system_quit_ingest_picked_save`) and then reopens the window as the normal
// slot view, so the "pick file -> pick character" flow never leaves the game's visual system.
//
// The only input this window gives the DLL is ROW ACTIVATION: `system_quit_profile_load_activate_hook`
// intercepts `CS::ProfileLoadDialog` vtable slot 20 (`0x9a4670`) and reads the highlighted list
// index out of `dialog+0xb0c`. Cursor movement, back and every other press stay inside the game's
// own list widget. Directory and drive browse actions are row/cell activations; overflow scrolling is
// handled by held-edge restaging so it does not consume visible rows.

/// Action object of the "Load Character from File" row; `system_quit_open_profile_load_dialog` derives
/// the System dialog (action+0x8), submit queue and window list from it on every (re)submit.
pub(crate) use er_telemetry::counters::SAVE_PICKER_ACTION_OBJ;
pub(crate) use er_telemetry::counters::SAVE_PICKER_CANCEL_COUNT;
/// 1 while the live picker is the save-DESTINATION chooser (save-game-flow WP3) instead of the
/// load-source browser: `[ new ]` is the initial selection (row 1 when drives occupy row 0), and
/// activation feeds the save flow.
pub(crate) use er_telemetry::counters::SAVE_PICKER_DEST_MODE;
/// 1 while the live `05_010_ProfileSelect` window is OUR file-picker (rows = directory listing).
/// 0 when it is the normal character-slot view.
pub(crate) use er_telemetry::counters::SAVE_PICKER_MODE_ACTIVE;
/// Diagnostics / telemetry oracles.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_COUNT;
/// 1 = a file was ingested from the picker; the menu-pump Run hook must resubmit `05_010` as the
/// NORMAL slot view (picker mode already cleared) so the user picks a character slot next.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_SLOTS_PENDING;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_REJECT_COUNT;
/// Dialog whose row list must be rebuilt in menu-pump ownership (0 = none). Set by a
/// navigation/cell activation after restaging records; consumed by the Run hook.
pub(crate) use er_telemetry::counters::SAVE_PICKER_REBUILD_PENDING_DIALOG;
/// 1 = the picker window was closed for a directory/page change; the menu-pump Run hook must
/// resubmit a fresh `05_010` job (records already restaged) instead of restoring the System UI.
pub(crate) use er_telemetry::counters::SAVE_PICKER_REOPEN_PENDING;
pub(crate) use er_telemetry::counters::SAVE_PICKER_REPOPULATE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_RESUBMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_STAGED_ROW_COUNT;
/// System/Quit dialog the live picker window was submitted from; the menu-pump resubmit reopens
/// through it (the destination picker is opened by the save flow, which has no row action object).
/// Do not use this as the live `05_010_ProfileSelect` dialog: cursor/rebuild work uses
/// `SYSTEM_QUIT_PROFILE_SELECT_WINDOW`, which is populated from the `05_010` MenuWindowJob owner.
pub(crate) use er_telemetry::counters::SAVE_PICKER_SYSTEM_DIALOG;

/// Windows-form (`Z:\...`) string for a possibly Linux-form absolute path; drive-prefixed paths
/// pass through with separators normalized. String twin of `system_quit_path_for_windows`.
pub(crate) fn save_picker_windows_path_string(path: &str) -> String {
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
pub(crate) fn save_picker_start_dir() -> Option<PathBuf> {
    if let Some(preferred) = crate::config::preferred_save_picker_dir_now()
        && let Some(text) = preferred.to_str()
    {
        let windows = PathBuf::from(save_picker_windows_path_string(text));
        if windows.is_dir() {
            return Some(windows);
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
///     for the staged ProfileSummary prefix. Cursor values read from the live `05_010_ProfileSelect`
///     dialog use the same dense row index; do not read the parent System/Quit dialog and try to
///     compensate for the resulting garbage offset.
pub(crate) unsafe fn save_picker_write_row_records(
    model: &crate::experiments::save_picker::SavePickerModel,
    summary: usize,
) -> usize {
    let visible = model
        .visible_row_count()
        .min(TITLE_PROFILE_SLOT_COUNT)
        .min(crate::experiments::save_picker::PICKER_ROW_COUNT);
    unsafe {
        for (slot, face_hash) in PROFILE_PREVIEW_FACE_HASH
            .iter()
            .enumerate()
            .take(TITLE_PROFILE_SLOT_COUNT)
        {
            let record = profile_summary_record_address(summary, slot);
            core::ptr::write_bytes(record as *mut u8, 0, PROFILE_SUMMARY_RECORD_STRIDE);
            face_hash.store(0, Ordering::SeqCst);
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
pub(crate) unsafe fn save_picker_stage_row_records(
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
        "save-picker: staged {staged} occupied row records ({} slots left unoccupied) dir='{}' scroll={}/{} entries={} drives={}",
        TITLE_PROFILE_SLOT_COUNT.saturating_sub(staged),
        model.current_dir().display(),
        model.scroll_offset(),
        model.scroll_max(),
        model.entry_count(),
        model.drive_count()
    ));
    true
}

/// Open the LOAD-source picker from the "Load Character from File" row action (menu thread). Which
/// surface that is -- this in-game browser or the OS file dialog -- is decided in one place,
/// [`open_picker_for_intent`]; the signature and the four call sites are unchanged.
pub(crate) unsafe fn system_quit_open_save_picker_menu(action_obj: usize) -> PickerOpenOutcome {
    unsafe { open_picker_for_intent(PickerOpenRequest::LoadSource { action_obj }) }
}

/// Open the IN-GAME file picker (menu thread). Mirrors the old OS-picker preflight (restore stale
/// preview, arm the active save snapshot), then stages the browse rows and submits the
/// `05_010_ProfileSelect` window.
pub(crate) unsafe fn system_quit_open_save_picker_menu_in_game(action_obj: usize) -> bool {
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("save-picker: refused to open -- {reason}"));
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

/// Open the save-DESTINATION chooser for the Save Game flow (save-game-flow WP3). Menu-pump
/// owned: called from `system_quit_menu_window_run_post` after the tick stages
/// `SAVE_DEST_OPEN_PICKER_PENDING`. Which surface opens is decided in one place,
/// [`open_picker_for_intent`]; the signature and the call site are unchanged.
pub(crate) unsafe fn system_quit_open_save_dest_picker(system_dialog: usize) -> PickerOpenOutcome {
    unsafe { open_picker_for_intent(PickerOpenRequest::SaveDestination { system_dialog }) }
}

/// Open the IN-GAME `05_010` picker as the save-destination chooser -- the same submit context
/// the load picker's resubmit uses.
///
/// Differences from the load-source picker, all deliberate:
///   * start dir = the LOADED save's own directory, not the remembered preferred dir -- "save
///     next to the save you loaded" is the expected default and the remembered dir belongs to the
///     load flow. Since the Save Game row press opens this browser with nothing in front of it,
///     that folder is also the first thing the user sees, so it has to be the one where both
///     answers -- a fresh file, or the save they are playing -- are one press away;
///   * NO save-swap byte preview is armed: nothing foreign is previewed here, and the safety
///     snapshot of the live save is taken later, at the fire gate, by `save_dest_arm_redirect`;
///   * the model carries the loaded save's filename so the `[ new ]` row writes that leaf, and its
///     full path so that row is marked `[CURRENT]` in the listing.
pub(crate) unsafe fn system_quit_open_save_dest_picker_in_game(system_dialog: usize) -> bool {
    const HEAP_LO: usize = 0x10000;
    if system_dialog < HEAP_LO || system_dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-dest-picker: refused to open -- System dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    let Some(SaveDestOrigin {
        start_dir,
        loaded_file_name,
        loaded_path,
    }) = save_dest_start_dir()
    else {
        return false;
    };
    unsafe { system_quit_save_swap_restore_profile_summary("save-dest-picker-open") };
    // Same mode-locked filter as the load picker: the destination list shows the containers the
    // active runtime flavor understands.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-save-dest-picker-open");
    let extensions: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    let model = crate::experiments::save_picker::SavePickerModel::open_destination(
        &start_dir,
        extensions,
        &loaded_file_name,
        &loaded_path,
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

/// Atomic stage edge used by menu-thread destination-picker decisions.
///
/// The game task owns the save-flow stage machine, but a picked destination is delivered on the
/// menu thread. If the game task has already moved the flow out of the destination-browser stage
/// (for example a timeout/abort path), the picker must not resurrect it by blindly storing a new
/// stage. Compare-and-swap against the browser stage and reset ticks only on success.
pub(crate) fn save_flow_menu_stage_cas(
    stage_word: &std::sync::atomic::AtomicUsize,
    ticks_word: &std::sync::atomic::AtomicUsize,
    expected: usize,
    stage: usize,
) -> Result<usize, usize> {
    let previous =
        stage_word.compare_exchange(expected, stage, Ordering::SeqCst, Ordering::SeqCst)?;
    ticks_word.store(0, Ordering::SeqCst);
    Ok(previous)
}

/// Enter a save-flow stage from the menu thread only if the game task has not already left the
/// expected stage.
pub(crate) fn save_flow_menu_enter_stage(expected: usize, stage: usize, reason: &str) -> bool {
    match save_flow_menu_stage_cas(&SAVE_FLOW_STAGE, &SAVE_FLOW_STAGE_TICKS, expected, stage) {
        Ok(previous) => {
            append_autoload_debug(format_args!(
                "save-flow: menu stage {previous} -> {stage} ({reason})"
            ));
            true
        }
        Err(actual) => {
            append_autoload_debug(format_args!(
                "save-flow: menu stage transition REFUSED expected={expected} actual={actual} target={stage} ({reason}); the destination decision is stale and nothing will be written from it"
            ));
            false
        }
    }
}

#[cfg(test)]
mod save_picker_row_hit_tests {
    use super::save_picker_row_from_stage_y;
    use er_gfx::title_05_010::{
        COMPACT_ROW_PITCH_PX, COMPACT_SCROLLBAR_TOP_Y_PX, COMPACT_VISIBLE_ROW_COUNT,
    };

    const TOP: f32 = COMPACT_SCROLLBAR_TOP_Y_PX as f32;
    const PITCH: f32 = COMPACT_ROW_PITCH_PX as f32;

    #[test]
    fn each_row_owns_exactly_one_pitch() {
        for row in 0..COMPACT_VISIBLE_ROW_COUNT as usize {
            let top_edge = TOP + PITCH * row as f32;
            assert_eq!(save_picker_row_from_stage_y(top_edge), Some(row));
            assert_eq!(
                save_picker_row_from_stage_y(top_edge + PITCH * 0.5),
                Some(row)
            );
            assert_eq!(
                save_picker_row_from_stage_y(top_edge + PITCH - 0.01),
                Some(row)
            );
        }
    }

    /// A click above or below the list must select NOTHING. Clamping to the nearest row instead
    /// would move the selection -- and therefore the game's activation -- for a click that never
    /// touched a row, on a screen whose rows load and overwrite saves.
    #[test]
    fn a_point_outside_the_band_hits_no_row() {
        assert_eq!(save_picker_row_from_stage_y(TOP - 0.01), None);
        let bottom = TOP + PITCH * COMPACT_VISIBLE_ROW_COUNT as f32;
        assert_eq!(save_picker_row_from_stage_y(bottom), None);
        assert_eq!(save_picker_row_from_stage_y(bottom + 500.0), None);
    }

    #[test]
    fn a_non_finite_point_hits_no_row() {
        assert_eq!(save_picker_row_from_stage_y(f32::NAN), None);
        assert_eq!(save_picker_row_from_stage_y(f32::INFINITY), None);
        assert_eq!(save_picker_row_from_stage_y(f32::NEG_INFINITY), None);
    }
}

#[cfg(test)]
mod save_picker_menu_stage_transition_tests {
    use super::save_flow_menu_stage_cas;

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn stage_cas_resets_ticks_only_when_expected_stage_matches() {
        let stage = AtomicUsize::new(3);
        let ticks = AtomicUsize::new(41);

        assert_eq!(save_flow_menu_stage_cas(&stage, &ticks, 3, 8), Ok(3));
        assert_eq!(stage.load(Ordering::SeqCst), 8);
        assert_eq!(ticks.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stage_cas_refuses_stale_menu_thread_decisions() {
        let stage = AtomicUsize::new(9);
        let ticks = AtomicUsize::new(41);

        assert_eq!(save_flow_menu_stage_cas(&stage, &ticks, 3, 8), Err(9));
        assert_eq!(stage.load(Ordering::SeqCst), 9);
        assert_eq!(ticks.load(Ordering::SeqCst), 41);
    }
}

/// Handle a destination-browser activation (menu thread, from `save_picker_handle_activation`).
/// `target` already exists -> the overwrite confirm; otherwise the commit is staged and the picker
/// closes so the save-flow tick can close the menus and fire.
///
/// THE ROUTE IS DECIDED BY THE TARGET, NOT BY WHICH ROW WAS PRESSED. `[ new ]` gets no exemption:
/// it resolves to the loaded save's own leaf in the browsed folder, and in the folder the browser
/// OPENS IN that leaf is the loaded save itself -- so pressing `[ new ]` there is an overwrite and
/// confirms like any other. The only rows that skip the question are the ones whose target does
/// not exist, where there is nothing to warn about.
pub(crate) unsafe fn save_dest_handle_picked_target(
    dialog: usize,
    target: PathBuf,
    source: &'static str,
) {
    unsafe {
        match save_dest_route_picked_target(&target) {
            DestRoute::ConfirmOverwrite => {
                SAVE_DEST_TARGET_EXISTING_COUNT.fetch_add(1, Ordering::SeqCst);
                // NO CONFIRM MEANS NO OVERWRITE. On a build whose MessageBoxBuilder recipe failed its
                // prologue check the question cannot be asked, and the answer to "may I destroy this
                // file without asking" is no. The user stays in the browser and can still save to a
                // free name; the refusal is counted so a run can tell it from a decline.
                if !save_flow_box_recipe_available() {
                    SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT.fetch_add(1, Ordering::SeqCst);
                    save_picker_set_visible_status(er_save_picker::PickerStatusMessage::new(
                        "CANNOT CONFIRM OVERWRITE",
                        "This build cannot show the overwrite prompt; choose a new file instead.",
                    ));
                    append_autoload_debug(format_args!(
                        "save-dest: REFUSED to overwrite '{}' (source={source}) -- the overwrite confirm cannot be built on this build, and an unconfirmed overwrite is not something this flow performs. Staying in the destination list with visible reason; a new file name still saves",
                        target.display()
                    ));
                    return;
                }
                save_dest_set_target(target, source);
                // The confirm is hosted by the PICKER dialog (the game raises its own confirms over
                // 05_010 the same way), so it does not contend with the System dialog queue that owns
                // the picker window job. Submitted inline here (menu thread); a not-ready queue leaves
                // the pending latch for the next menu pump.
                save_flow_box_set_host_dialog(dialog);
                SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_OVERWRITE_FILE, Ordering::SeqCst);
                if !save_flow_menu_enter_stage(
                    SAVE_FLOW_STAGE_DEST_BROWSE,
                    SAVE_FLOW_STAGE_OVERWRITE_CONFIRM,
                    "picked existing destination -> overwrite confirm",
                ) {
                    save_flow_box_clear();
                    save_dest_clear_target("stale overwrite-confirm stage transition");
                    return;
                }
                if save_flow_submit_box(SAVE_FLOW_BOX_OVERWRITE_FILE) {
                    SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
                }
            }
            DestRoute::CommitDirect => {
                SAVE_DEST_TARGET_NEW_COUNT.fetch_add(1, Ordering::SeqCst);
                save_dest_set_target(target, source);
                save_dest_stage_commit_and_close_picker(dialog, "new-file");
            }
        }
    }
}

/// Stage the destination commit and close the browser. The save-flow tick takes over once the
/// picker window has finished tearing down (the native close also restores the user's real
/// ProfileSummary rows and re-shows the System windows, which is exactly the state the close-all
/// sequence expects).
pub(crate) unsafe fn save_dest_stage_commit_and_close_picker(dialog: usize, reason: &str) {
    if !save_flow_menu_enter_stage(
        SAVE_FLOW_STAGE_DEST_BROWSE,
        SAVE_FLOW_STAGE_DEST_BROWSE,
        "picked free destination -> commit",
    ) {
        SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
        save_dest_clear_target("stale destination-commit stage transition");
        return;
    }
    SAVE_DEST_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_DEST_COMMIT_PENDING.store(1, Ordering::SeqCst);
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
    if save_picker_path_editor_active() {
        // 02_990 owns Accept/Back while active. Never let the frozen ProfileSelect row interpret the
        // same edge as a browse action.
        return 0;
    }
    let Some(model_row) = save_picker_model_row_from_native_cursor(cursor) else {
        append_autoload_debug(format_args!(
            "save-picker: activation ignored invalid native cursor={cursor}"
        ));
        return 0;
    };
    let mut open_path_editor = false;
    let activation = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_mut() else {
            append_autoload_debug(format_args!(
                "save-picker: activation with no model (native_cursor={cursor} model_row={model_row}); ignoring"
            ));
            return 0;
        };
        let pending_cell = SAVE_PICKER_DRIVE_STRIP_PENDING_CELL
            .swap(SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL, Ordering::SeqCst);
        if model.drive_row() == Some(model_row) {
            let cell_count = model.drive_strip_cell_count();
            if pending_cell == SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING {
                model.focus_current_path_from_drive_strip();
                open_path_editor = true;
                PickerActivation::Ignored
            } else if let Some(cell) =
                save_picker_pending_drive_strip_cell(pending_cell, cell_count)
            {
                if model.activate_drive_strip_cell(cell) {
                    append_autoload_debug(format_args!(
                        "save-picker: native drive-row activation selected pending cell={cell} native_cursor={cursor} model_row={model_row} cells={cell_count}"
                    ));
                    PickerActivation::Repopulate
                } else {
                    append_autoload_debug(format_args!(
                        "save-picker: native drive-row activation rejected pending cell={cell} native_cursor={cursor} model_row={model_row} cells={cell_count}"
                    ));
                    PickerActivation::Ignored
                }
            } else if model.drive_strip_focus()
                == Some(er_save_picker::DriveStripFocus::CurrentPath)
            {
                open_path_editor = true;
                PickerActivation::Ignored
            } else {
                // Accept belongs to the focused sub-control. A bare row activation while a drive
                // cell owns focus is inert; Right from the final drive or pointer hover/click moves
                // focus to CurrentPath first.
                PickerActivation::Ignored
            }
        } else {
            model.activate(model_row)
        }
    };
    if open_path_editor {
        save_picker_request_path_editor(dialog);
        append_autoload_debug(format_args!(
            "save-picker-path: native drive-row activation requested editor dialog=0x{dialog:x} native_cursor={cursor} model_row={model_row}"
        ));
        return 0;
    }
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
        PickerActivation::PickedFile(path) if SAVE_PICKER_DEST_MODE.load(Ordering::SeqCst) != 0 => {
            // DESTINATION browser: an existing container was picked as the save target, so the
            // final overwrite confirm decides. No ingest/preview -- nothing is being loaded.
            unsafe { save_dest_handle_picked_target(dialog, path, "picked-file") };
            0
        }
        PickerActivation::PickedNewFile(path) => {
            // `[ new ]`: save into the browsed folder under the loaded save's own filename. If
            // that file already exists there, fall into the Box3 overwrite confirm rather than
            // silently clobbering it.
            unsafe { save_dest_handle_picked_target(dialog, path, "new-row") };
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
                save_picker_set_visible_status(
                    er_save_picker::PickRejection::PathNotUtf8.status_message("SL2"),
                );
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
pub(crate) unsafe fn save_picker_native_close(dialog: usize, reason: &str) {
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

const PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET: usize = 0xa38;
const GRID_CONTROL_SCROLLBAR_OFFSET: usize = 0x1a8;
const PROFILE_LOAD_DIALOG_SCROLLBAR_OFFSET: usize =
    PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET + GRID_CONTROL_SCROLLBAR_OFFSET;
const MENU_ITEM_LIST_CURSOR_GETTER_RVA: usize = 0x739e20;
/// The field `MENU_ITEM_LIST_CURSOR_GETTER_RVA` reads: `FUN_140739e20` is exactly
/// `*(undefined4 *)(param_1 + 0xd4)`. `GridControl::HandleMouse` writes the same field on a hit.
const MENU_ITEM_LIST_CURSOR_FIELD_OFFSET: usize = 0xd4;
/// `FUN_14073bc10(grid, index)` -- the native SELECT-INDEX primitive, not a field poke.
///
/// Writing `+0xd4` directly changes which row is selected without any of the side effects that
/// make the selection VISIBLE: the highlight stays where it was while the logical cursor moves
/// somewhere else (reported 2026-08-12 as the wheel scrolling rows "but the chrome doesn't travel
/// with it"). This is the function the game's own mouse handler calls after resolving a hit
/// (`GridControl::HandleMouse` -> `FUN_140736c90` returns the cell -> `FUN_14073bc10(grid, cell)`),
/// and it bounds-checks the index, runs `FUN_140739830`, and only then writes `+0xd4`.
///
/// Address byte-verified against `eldenring-deobf.bin`: the prologue
/// `48 89 5c 24 18 89 54 24 10 57 48 83 ec 20 44 8b 99 dc 00 00 00` occurs exactly ONCE in the
/// image, at `0x14073bc10` (1.16.2 dump VA == deobf VA == runtime VA, shift 0).
const MENU_ITEM_LIST_SET_CURSOR_RVA: usize = 0x73bc10;
/// The grid's ensure-visible bases, column and row: `FUN_140739830` measures the target as
/// `index % cols - [+0xe0]` and `index / cols - [+0x348]`.
///
/// `+0x348` IS THE SCROLLBAR POSITION, not a private grid field. The scrollbar control is embedded
/// at `grid+0x1a8` and `ScrollbarControl::SetPosition` (`FUN_14074db60`) writes `scrollbar+0x1a0`
/// -- and `0x1a8 + 0x1a0 == 0x348`. The native list scrolls its view BY that position: the game's
/// design is one item array plus a moving window.
///
/// The picker inverts that: it stages only the ten VISIBLE rows and scrolls by re-staging them, so
/// its cursor indices are always 0..9 while the scrollbar carries a model-space position (row 8 of
/// 32). Those two spaces disagree, so any native ensure-visible decides the selection is far above
/// the window and scrolls to it -- resetting the scrollbar to 0, which is the list "re-orienting"
/// under a hover and one wheel notch travelling two rows (live log 2026-08-12: base 8 while
/// scroll_offset=8/32, reset to 0 by the select call).
const GRID_CONTROL_VIEW_COL_BASE_OFFSET: usize = 0xe0;
const GRID_CONTROL_VIEW_ROW_BASE_OFFSET: usize = 0x348;
/// Item count, columns and rows-per-view on the grid, read only to explain the index space in the
/// log (`FUN_14073bc10` bounds-checks the index against exactly these).
const GRID_CONTROL_ITEM_COUNT_OFFSET: usize = 0xd0;
const GRID_CONTROL_COLUMNS_OFFSET: usize = 0xd8;
const GRID_CONTROL_ROWS_OFFSET: usize = 0xdc;
/// Selection row observed at the END of the previous edge-scroll pump tick, i.e. before the native
/// list consumed this tick's key. `EDGE_SCROLL_NO_PREV_CURSOR` means "no usable prior sample".
static SAVE_PICKER_EDGE_SCROLL_PREV_CURSOR: AtomicUsize =
    AtomicUsize::new(EDGE_SCROLL_NO_PREV_CURSOR);
const EDGE_SCROLL_NO_PREV_CURSOR: usize = usize::MAX;
const SCROLLBAR_CONTROL_SET_TOTAL_RVA: u32 = 0x74dad0;
const SCROLLBAR_CONTROL_SET_POSITION_RVA: u32 = 0x74db60;
static SAVE_PICKER_SCROLLBAR_LAST_SYNC: AtomicUsize = AtomicUsize::new(usize::MAX);
static SAVE_PICKER_SCROLLBAR_DEAD_PROXY_SKIPS: AtomicUsize = AtomicUsize::new(0);
const MENU_VIEWER_EVENT_POINT_RVA: usize = 0x757af0;
const PROFILE_SELECT_MOVIE_WIDTH_PX: f32 = 1920.0;
const PROFILE_SELECT_MOVIE_HEIGHT_PX: f32 = 1080.0;
/// `ProfileList` is placed at root movie x=960 and every nested ItemList/row placement has identity
/// x; `save_picker_client_point_to_movie_stage` subtracts that same half-width. Drive-cell row-local
/// x is therefore already the mouse stage x -- no guessed scale/offset belongs here. The native
/// frame's visible shape starts about four pixels before the authored text-field x.
const DRIVE_STRIP_HIT_LEFT_PX: f32 = er_gfx::title_05_010::DRIVE_CELL_FIRST_X_PX - 4.0;
const DRIVE_STRIP_CELL_PITCH_PX: f32 = er_gfx::title_05_010::DRIVE_CELL_PITCH_PX;
const DRIVE_STRIP_CELL_HIT_WIDTH_PX: f32 = DRIVE_STRIP_CELL_PITCH_PX;
const _: () =
    assert!(er_save_picker::DRIVE_STRIP_MAX_CELLS <= er_gfx::title_05_010::DRIVE_CELL_CAPACITY);
/// Live `05_010_ProfileSelect` cursor values are already staged model-row indices. The old +2
/// observation came from reading the parent System/Quit dialog, not the live ProfileSelect dialog.
const PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET: i32 = 0;
const SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL: usize = usize::MAX;
const SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING: usize = usize::MAX - 1;
static SAVE_PICKER_DRIVE_STRIP_PENDING_CELL: AtomicUsize =
    AtomicUsize::new(SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL);
const SAVE_PICKER_DRIVE_STRIP_LBUTTON_MASK: usize = 1 << 0;
const SAVE_PICKER_DRIVE_STRIP_LEFT_MASK: usize = 1 << 1;
const SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK: usize = 1 << 2;
static SAVE_PICKER_DRIVE_STRIP_INPUT_DOWN_MASK: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_DRIVE_STRIP_LAST_POINTER_BITS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

unsafe fn save_picker_event_point(event: usize) -> Option<(f32, f32)> {
    if event == 0 {
        return None;
    }
    let Ok(base) = game_module_base() else {
        return None;
    };
    let point_fn: unsafe extern "system" fn(usize, *mut u64) -> *mut u64 =
        unsafe { std::mem::transmute(base + MENU_VIEWER_EVENT_POINT_RVA) };
    let mut packed = 0_u64;
    unsafe { point_fn(event, &mut packed as *mut u64) };
    let x = f32::from_bits(packed as u32);
    let y = f32::from_bits((packed >> 32) as u32);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

pub(crate) unsafe fn save_picker_note_drive_strip_click_event(event: usize) {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 || save_picker_path_editor_active() {
        return;
    }
    let dialog = save_picker_live_profile_dialog();
    if dialog == 0 {
        return;
    }
    let cursor = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1);
    let Some(model_row) = save_picker_model_row_from_native_cursor(cursor) else {
        return;
    };
    let Some((x, y)) = (unsafe { save_picker_event_point(event) }) else {
        return;
    };
    let cell_count = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return;
        };
        if model.drive_row() != Some(model_row) {
            return;
        }
        model.drive_strip_cell_count()
    };
    if cell_count == 0 {
        return;
    }
    if let Some(cell) = save_picker_drive_strip_cell_from_x(x, cell_count) {
        SAVE_PICKER_DRIVE_STRIP_PENDING_CELL.store(cell, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: drive-strip click native_cursor={cursor} model_row={model_row} x={x:.1} y={y:.1} cells={cell_count} -> cell={cell}"
        ));
    } else if save_picker_current_path_contains_x(x) {
        SAVE_PICKER_DRIVE_STRIP_PENDING_CELL.store(
            SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING,
            Ordering::SeqCst,
        );
        append_autoload_debug(format_args!(
            "save-picker-path: click armed native text editor native_cursor={cursor} model_row={model_row} x={x:.1} y={y:.1}"
        ));
    } else {
        append_autoload_debug(format_args!(
            "save-picker: drive-row click outside drive/path controls native_cursor={cursor} model_row={model_row} x={x:.1} y={y:.1} cells={cell_count}"
        ));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DriveStripPointerHit {
    Cell(usize),
    CurrentPath,
}

fn save_picker_drive_strip_hit_from_x(x: f32, cell_count: usize) -> Option<DriveStripPointerHit> {
    save_picker_drive_strip_cell_from_x(x, cell_count)
        .map(DriveStripPointerHit::Cell)
        .or_else(|| {
            save_picker_current_path_contains_x(x).then_some(DriveStripPointerHit::CurrentPath)
        })
}

fn save_picker_current_path_contains_x(x: f32) -> bool {
    (er_gfx::title_05_010::CURRENT_PATH_X_PX
        ..er_gfx::title_05_010::CURRENT_PATH_X_PX + er_gfx::title_05_010::CURRENT_PATH_WIDTH_PX)
        .contains(&x)
}

fn save_picker_pending_drive_strip_cell(pending_cell: usize, cell_count: usize) -> Option<usize> {
    (pending_cell != SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL && pending_cell < cell_count)
        .then_some(pending_cell)
}

fn save_picker_model_row_from_native_cursor(cursor: i32) -> Option<usize> {
    let row = cursor.checked_sub(PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET)?;
    (row >= 0 && (row as usize) < crate::experiments::save_picker::PICKER_ROW_COUNT)
        .then_some(row as usize)
}

fn save_picker_live_profile_dialog() -> usize {
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst)
}

/// Final derived vtable installed by `FUN_140875590` on
/// `BasicViewItemList<MenuSaveDataSummary,10>`. During destruction it regresses through
/// `0x2ad53c8` and `0x2a9e598`; slot `+8` is then pure virtual. The records-changed rebuild calls
/// that slot at `FUN_1409a2cf0+0x3d`, so the exact derived vtable is a hard precondition.
const PROFILE_SELECT_DERIVED_LIST_VTABLE_RVA: usize = 0x2ad5400;
const PROFILE_LOAD_DIALOG_STORED_LIST_OFFSET: usize = 0x1260;

fn save_picker_rebuild_target_is_live(
    dialog_vtable: usize,
    list_vtable: usize,
    game_base: usize,
) -> bool {
    dialog_vtable == game_base + er_title_flow::PROFILE_LOAD_DIALOG_VTABLE_RVA
        && list_vtable == game_base + PROFILE_SELECT_DERIVED_LIST_VTABLE_RVA
}

unsafe fn save_picker_rebuild_profile_dialog_now(dialog: usize, reason: &str) -> bool {
    if dialog == 0 || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 {
        return false;
    }
    let Ok(base) = game_module_base() else {
        return false;
    };
    let dialog_vtable = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    let list_vtable =
        unsafe { safe_read_usize(dialog + PROFILE_LOAD_DIALOG_STORED_LIST_OFFSET) }.unwrap_or(0);
    if !save_picker_rebuild_target_is_live(dialog_vtable, list_vtable, base) {
        append_autoload_debug(format_args!(
            "save-picker: dropped in-place list rebuild for terminal/unbound dialog=0x{dialog:x} reason={reason} dialog_vt=0x{dialog_vtable:x} list_vt=0x{list_vtable:x}; native rebuild would purecall at game+0x9a2d2d"
        ));
        return false;
    }
    if let Ok(rebuild_addr) = game_rva(PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA) {
        let rebuild: unsafe extern "system" fn(usize) =
            unsafe { std::mem::transmute(rebuild_addr) };
        unsafe { rebuild(dialog) };
        append_autoload_debug(format_args!(
            "save-picker: menu-pump in-place list rebuild dialog=0x{dialog:x} reason={reason} via 0x{rebuild_addr:x}"
        ));
        true
    } else {
        SAVE_PICKER_REOPEN_PENDING.store(1, Ordering::SeqCst);
        unsafe { save_picker_native_close(dialog, reason) };
        false
    }
}

#[cfg(test)]
mod rebuild_liveness_tests {
    use super::*;

    #[test]
    fn rebuild_requires_the_live_dialog_and_final_derived_list_vtables() {
        let base = 0x140000000;
        let dialog = base + er_title_flow::PROFILE_LOAD_DIALOG_VTABLE_RVA;
        let list = base + PROFILE_SELECT_DERIVED_LIST_VTABLE_RVA;
        assert!(save_picker_rebuild_target_is_live(dialog, list, base));
        assert!(!save_picker_rebuild_target_is_live(
            dialog,
            base + 0x2ad53c8,
            base
        ));
        assert!(!save_picker_rebuild_target_is_live(
            dialog,
            base + 0x2a9e598,
            base
        ));
        assert!(!save_picker_rebuild_target_is_live(0, list, base));
    }
}

fn save_picker_client_point_to_movie_stage(
    client_x: f32,
    client_y: f32,
    client_width: f32,
    client_height: f32,
) -> Option<(f32, f32)> {
    if !(client_x.is_finite()
        && client_y.is_finite()
        && client_width.is_finite()
        && client_height.is_finite())
        || client_width <= 0.0
        || client_height <= 0.0
    {
        return None;
    }
    // 05_010 is authored as a fixed 1920x1080 movie. The user's actual window/monitor resolution is
    // deliberately not assumed: map through the movie rectangle fitted into the live client area,
    // preserving aspect ratio and removing any letterbox/pillarbox margin first.
    let movie_aspect = PROFILE_SELECT_MOVIE_WIDTH_PX / PROFILE_SELECT_MOVIE_HEIGHT_PX;
    let client_aspect = client_width / client_height;
    let (content_x, content_y, content_w, content_h) = if client_aspect > movie_aspect {
        let content_w = client_height * movie_aspect;
        (
            (client_width - content_w) * 0.5,
            0.0,
            content_w,
            client_height,
        )
    } else {
        let content_h = client_width / movie_aspect;
        (
            0.0,
            (client_height - content_h) * 0.5,
            client_width,
            content_h,
        )
    };
    let in_content_x = client_x - content_x;
    let in_content_y = client_y - content_y;
    if in_content_x < 0.0
        || in_content_y < 0.0
        || in_content_x >= content_w
        || in_content_y >= content_h
    {
        return None;
    }
    let stage_x = (in_content_x / content_w) * PROFILE_SELECT_MOVIE_WIDTH_PX
        - PROFILE_SELECT_MOVIE_WIDTH_PX * 0.5;
    let stage_y = (in_content_y / content_h) * PROFILE_SELECT_MOVIE_HEIGHT_PX
        - PROFILE_SELECT_MOVIE_HEIGHT_PX * 0.5;
    Some((stage_x, stage_y))
}

/// The live pointer in movie stage coordinates, or `None` when it is outside the movie's content
/// box. Shared by the drive strip and the row hit test so both read the same space.
fn save_picker_stage_cursor() -> Option<(f32, f32)> {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetForegroundWindow, GetWindowRect,
    };

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        return None;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let width = (rect.right - rect.left).max(1) as f32;
    let height = (rect.bottom - rect.top).max(1) as f32;
    let window_x = point.x - rect.left;
    let window_y = point.y - rect.top;
    save_picker_client_point_to_movie_stage(window_x as f32, window_y as f32, width, height)
}

/// Which visible list row a stage-space Y lands on.
///
/// The list band is authored geometry rather than a guess: it starts at
/// `COMPACT_SCROLLBAR_TOP_Y_PX` and runs `COMPACT_VISIBLE_ROW_COUNT` rows of
/// `COMPACT_ROW_PITCH_PX` -- the same span the scrollbar track covers, which
/// `stats_panel_output_scrollbar_track_and_thumb_span_the_visible_rows` already asserts. Rows are
/// native view rows `0..9`, which are model rows too (`PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET` = 0).
fn save_picker_row_from_stage_y(stage_y: f32) -> Option<usize> {
    if !stage_y.is_finite() {
        return None;
    }
    let local = stage_y - er_gfx::title_05_010::COMPACT_SCROLLBAR_TOP_Y_PX as f32;
    if local < 0.0 {
        return None;
    }
    let row = (local / er_gfx::title_05_010::COMPACT_ROW_PITCH_PX as f32).floor();
    if row < 0.0 {
        return None;
    }
    let row = row as usize;
    (row < er_gfx::title_05_010::COMPACT_VISIBLE_ROW_COUNT as usize).then_some(row)
}

fn save_picker_drive_strip_hit_from_live_cursor(
    cell_count: usize,
) -> Option<(DriveStripPointerHit, f32, f32)> {
    let (stage_x, stage_y) = save_picker_stage_cursor()?;
    save_picker_drive_strip_hit_from_x(stage_x, cell_count).map(|hit| (hit, stage_x, stage_y))
}

fn save_picker_drive_strip_cell_from_x(x: f32, cell_count: usize) -> Option<usize> {
    if cell_count == 0 || !x.is_finite() {
        return None;
    }
    let local_x = x - DRIVE_STRIP_HIT_LEFT_PX;
    if local_x < 0.0 {
        return None;
    }
    let cell = (local_x / DRIVE_STRIP_CELL_PITCH_PX).floor() as usize;
    if cell >= cell_count {
        return None;
    }
    let in_cell_x = local_x - cell as f32 * DRIVE_STRIP_CELL_PITCH_PX;
    (in_cell_x < DRIVE_STRIP_CELL_HIT_WIDTH_PX).then_some(cell)
}

#[cfg(test)]
mod drive_strip_hit_tests {
    use super::*;
    use er_save_picker::DRIVE_STRIP_MAX_CELLS;

    #[test]
    fn every_possible_drive_cell_lives_in_the_clickable_player_name_band() {
        assert_eq!(
            save_picker_drive_strip_cell_from_x(DRIVE_STRIP_HIT_LEFT_PX + 0.1, 26),
            Some(0)
        );
        assert_eq!(
            save_picker_drive_strip_cell_from_x(
                DRIVE_STRIP_HIT_LEFT_PX + 12.0 * DRIVE_STRIP_CELL_PITCH_PX + 0.1,
                26,
            ),
            Some(12)
        );
        assert_eq!(
            save_picker_drive_strip_cell_from_x(
                DRIVE_STRIP_HIT_LEFT_PX + 25.0 * DRIVE_STRIP_CELL_PITCH_PX + 0.1,
                26,
            ),
            Some(25)
        );
        assert_eq!(
            save_picker_drive_strip_cell_from_x(
                DRIVE_STRIP_HIT_LEFT_PX + 26.0 * DRIVE_STRIP_CELL_PITCH_PX,
                26,
            ),
            None
        );
    }

    #[test]
    fn native_drive_buttons_render_only_the_drive_name_and_use_color_for_selection() {
        let selected = save_picker_drive_cell_html_utf16(">C:<");
        let idle = save_picker_drive_cell_html_utf16("[S:]");
        let selected = String::from_utf16(&selected[..selected.len() - 1]).expect("valid UTF-16");
        let idle = String::from_utf16(&idle[..idle.len() - 1]).expect("valid UTF-16");
        assert!(selected.contains("C:"));
        assert!(selected.contains("size=\"20\""));
        assert!(!selected.contains(">>C:<"));
        assert!(selected.contains("#d8a052"));
        assert!(idle.contains("S:"));
        assert!(!idle.contains(">[S:]"));
        assert!(idle.contains("#8f887a"));
    }

    #[test]
    fn one_physical_arrow_source_produces_exactly_one_drive_action() {
        // Old path: async keyboard poll fired on tick one, then the DInput edge fired on tick two.
        // The async arrow is now ignored; only the native-input edge becomes an action.
        assert_eq!(
            drive_strip_pressed_mask(0, SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK, 0),
            0
        );
        assert_eq!(
            drive_strip_pressed_mask(
                SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK,
                SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK,
                crate::experiments::SAVE_PICKER_NAV_RIGHT_MASK,
            ),
            SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK
        );
    }

    #[test]
    fn activation_prefers_event_time_pending_drive_cell() {
        assert_eq!(save_picker_pending_drive_strip_cell(0, 3), Some(0));
        assert_eq!(save_picker_pending_drive_strip_cell(1, 3), Some(1));
        assert_eq!(save_picker_pending_drive_strip_cell(2, 3), Some(2));
        assert_eq!(save_picker_pending_drive_strip_cell(3, 3), None);
        assert_eq!(
            save_picker_pending_drive_strip_cell(SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL, 3),
            None
        );
    }

    #[test]
    fn complete_path_hit_target_is_distinct_from_drive_cells() {
        assert!(save_picker_current_path_contains_x(
            er_gfx::title_05_010::CURRENT_PATH_X_PX
        ));
        assert!(save_picker_current_path_contains_x(
            er_gfx::title_05_010::CURRENT_PATH_X_PX + er_gfx::title_05_010::CURRENT_PATH_WIDTH_PX
                - 0.1
        ));
        assert!(!save_picker_current_path_contains_x(
            er_gfx::title_05_010::CURRENT_PATH_X_PX + er_gfx::title_05_010::CURRENT_PATH_WIDTH_PX
        ));
        assert_eq!(
            save_picker_drive_strip_hit_from_x(
                er_gfx::title_05_010::CURRENT_PATH_X_PX + 1.0,
                DRIVE_STRIP_MAX_CELLS,
            ),
            Some(DriveStripPointerHit::CurrentPath)
        );
        assert_eq!(
            save_picker_pending_drive_strip_cell(
                SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING,
                DRIVE_STRIP_MAX_CELLS,
            ),
            None,
            "the path-editor sentinel must never alias a drive-cell selection"
        );
    }

    #[test]
    fn entering_path_edit_mode_immediately_hides_the_read_only_path_label() {
        save_picker_reset_path_editor_state();
        save_picker_request_path_editor(0x1234);
        assert_eq!(save_picker_current_path_text(0), Some(vec![0]));
        save_picker_reset_path_editor_state();
    }

    #[test]
    fn live_profile_select_cursor_is_the_model_row_index() {
        assert_eq!(save_picker_model_row_from_native_cursor(-1), None);
        assert_eq!(save_picker_model_row_from_native_cursor(0), Some(0));
        assert_eq!(save_picker_model_row_from_native_cursor(1), Some(1));
        assert_eq!(save_picker_model_row_from_native_cursor(9), Some(9));
        assert_eq!(save_picker_model_row_from_native_cursor(10), None);
    }

    fn client_x_for_stage_x(stage_x: f32, client_width: f32, client_height: f32) -> f32 {
        let movie_aspect = PROFILE_SELECT_MOVIE_WIDTH_PX / PROFILE_SELECT_MOVIE_HEIGHT_PX;
        let client_aspect = client_width / client_height;
        if client_aspect > movie_aspect {
            let content_w = client_height * movie_aspect;
            ((client_width - content_w) * 0.5)
                + ((stage_x + PROFILE_SELECT_MOVIE_WIDTH_PX * 0.5) / PROFILE_SELECT_MOVIE_WIDTH_PX)
                    * content_w
        } else {
            ((stage_x + PROFILE_SELECT_MOVIE_WIDTH_PX * 0.5) / PROFILE_SELECT_MOVIE_WIDTH_PX)
                * client_width
        }
    }

    #[test]
    fn live_cursor_mapping_uses_fixed_movie_stage_not_user_resolution() {
        let second_cell_x = DRIVE_STRIP_HIT_LEFT_PX + DRIVE_STRIP_CELL_PITCH_PX + 0.1;
        for (client_width, client_height) in [
            (1920.0, 1080.0),
            (2560.0, 1440.0),
            (3440.0, 1440.0),
            (1024.0, 768.0),
        ] {
            let client_x = client_x_for_stage_x(second_cell_x, client_width, client_height);
            let client_y = client_height * 0.5;
            let (stage_x, stage_y) = save_picker_client_point_to_movie_stage(
                client_x,
                client_y,
                client_width,
                client_height,
            )
            .expect("point should lie inside the fitted movie stage");
            assert!(
                (stage_x - second_cell_x).abs() < 0.02,
                "client {client_width}x{client_height} mapped x={stage_x}, not the fixed movie-stage boundary {second_cell_x}"
            );
            assert!(stage_y.abs() < 0.02);
            assert_eq!(save_picker_drive_strip_cell_from_x(stage_x, 26), Some(1));
        }
    }

    #[test]
    fn live_cursor_mapping_rejects_pillarbox_margin() {
        assert_eq!(
            save_picker_client_point_to_movie_stage(100.0, 720.0, 3440.0, 1440.0),
            None,
            "ultrawide pillarbox margin must not be treated as movie coordinates"
        );
    }
}

fn drive_strip_pressed_mask(prev_down: usize, down_mask: usize, nav_edges: usize) -> usize {
    let mut pressed = (down_mask & !prev_down) & SAVE_PICKER_DRIVE_STRIP_LBUTTON_MASK;
    if nav_edges & crate::experiments::SAVE_PICKER_NAV_LEFT_MASK != 0 {
        pressed |= SAVE_PICKER_DRIVE_STRIP_LEFT_MASK;
    }
    if nav_edges & crate::experiments::SAVE_PICKER_NAV_RIGHT_MASK != 0 {
        pressed |= SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK;
    }
    pressed
}

/// Menu-pump-owned drive-strip mouse/keyboard handling. The native ProfileSelect list exposes only
/// one hit target per row, so `[C:]  [S:]  [Z:]` can never be true native sub-buttons. While the
/// native cursor is on the drive row, sample input edges in the menu pump and mutate the picker model
/// directly: mouse uses the live X coordinate; Left/Right cycle to the adjacent drive.
pub(crate) unsafe fn save_picker_menu_pump_drive_strip_mouse() {
    if save_picker_path_editor_active() {
        SAVE_PICKER_DRIVE_STRIP_INPUT_DOWN_MASK.store(0, Ordering::SeqCst);
        let _ = crate::experiments::save_picker_take_user_nav_edges();
        return;
    }
    let dialog = save_picker_live_profile_dialog();
    if dialog == 0 || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 {
        SAVE_PICKER_DRIVE_STRIP_INPUT_DOWN_MASK.store(0, Ordering::SeqCst);
        SAVE_PICKER_DRIVE_STRIP_LAST_POINTER_BITS.store(u64::MAX, Ordering::SeqCst);
        let _ = crate::experiments::save_picker_take_user_nav_edges();
        return;
    }
    crate::experiments::ensure_save_picker_user_nav_input_hooks_installed();
    install_save_picker_set_cursor_hook();
    install_save_picker_wheel_delta_hook();
    install_save_picker_hit_test_hook();
    let mut down_mask = 0usize;
    if unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x01) < 0 } {
        down_mask |= SAVE_PICKER_DRIVE_STRIP_LBUTTON_MASK;
    }
    // Keyboard/controller arrows come ONLY from the DInput/XInput edge latch. Polling the same
    // physical key through GetAsyncKeyState as well delivered one action here and a second action
    // when the native-input edge arrived a few milliseconds later (runtime log 2026-08-08).
    let prev_down = SAVE_PICKER_DRIVE_STRIP_INPUT_DOWN_MASK.swap(down_mask, Ordering::SeqCst);
    // Left/right only: up/down belong to the edge-scroll pump in this same tick.
    let nav_edges = crate::experiments::save_picker_take_user_nav_edges_for(
        crate::experiments::SAVE_PICKER_NAV_LEFT_MASK
            | crate::experiments::SAVE_PICKER_NAV_RIGHT_MASK,
    );
    let pressed = drive_strip_pressed_mask(prev_down, down_mask, nav_edges);
    let Ok(base) = game_module_base() else {
        return;
    };
    let cursor_getter: unsafe extern "system" fn(usize) -> i32 =
        unsafe { std::mem::transmute(base + MENU_ITEM_LIST_CURSOR_GETTER_RVA) };
    let cursor = unsafe { cursor_getter(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET) };
    let Some(model_row) = save_picker_model_row_from_native_cursor(cursor) else {
        if pressed != 0 {
            append_autoload_debug(format_args!(
                "save-picker-nav: pressed_mask=0x{pressed:x} ignored invalid native_cursor={cursor}"
            ));
        }
        return;
    };
    let (on_drive_row, drive_row, drive_cell_count) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return;
        };
        let drive_row = model.drive_row();
        (
            drive_row == Some(model_row),
            drive_row,
            model.drive_strip_cell_count(),
        )
    };
    if pressed == 0 {
        let hover = on_drive_row
            .then(|| save_picker_drive_strip_hit_from_live_cursor(drive_cell_count))
            .flatten();
        let pointer_bits = hover
            .map(|(_, x, y)| u64::from(x.to_bits()) | (u64::from(y.to_bits()) << 32))
            .unwrap_or(u64::MAX);
        let moved = SAVE_PICKER_DRIVE_STRIP_LAST_POINTER_BITS.swap(pointer_bits, Ordering::SeqCst)
            != pointer_bits;
        if moved {
            let changed = {
                let mut guard = crate::experiments::save_picker::active_save_picker_lock();
                guard.as_mut().is_some_and(|model| match hover {
                    Some((DriveStripPointerHit::CurrentPath, _, _)) => {
                        model.focus_current_path_from_drive_strip()
                    }
                    Some((DriveStripPointerHit::Cell(_), _, _)) => {
                        model.focus_active_drive_from_drive_strip()
                    }
                    // Pointer hit NOTHING on the row. Treating that as a drive-cell hover used to
                    // yank focus back off CurrentPath on any jog through the 20px dead zone between
                    // the cell band and the path control -- and since the focused sub-control is
                    // also the row's native hit area, that re-locked the pointer out of the path.
                    None => false,
                })
            };
            if changed {
                let staged = {
                    let guard = crate::experiments::save_picker::active_save_picker_lock();
                    guard
                        .as_ref()
                        .is_some_and(|model| unsafe { save_picker_stage_row_records(model) })
                };
                if staged {
                    SAVE_PICKER_REBUILD_PENDING_DIALOG.store(dialog, Ordering::SeqCst);
                }
            }
        }
        return;
    }
    if !on_drive_row {
        // LEFT CLICK ON A LIST ROW = ACCEPT, on the row that was actually CLICKED -- and the ONLY
        // thing done here is moving the native selection onto that row. The game activates the click
        // itself; it simply had nothing to act on while the pointer could not reach the selection.
        //
        // Two things had to be unlearned to get here. Raising the Confirm menu event at
        // `CSMenuManImp+0x90+0x3d` did nothing at all: the write LANDED on every click
        // (`confirm_raised=true`) and the only `ProfileLoadDialog ACTIVATE` in that window arrived
        // 1.3 SECONDS later from a real key press, so that constant is not the id this dialog reads.
        // Calling the activation ourselves then worked far too well -- one click produced TWO
        // parent-folder steps, ours at `+114118ms` (`listed 'save-files'`) and the game's own at
        // `+114139ms` (`listed 'er-effects-rs'`), twenty milliseconds apart. That is the wheel's
        // double-scroll wearing a different hat, and on a screen whose rows load and overwrite saves
        // an uninvited second activation is a hazard rather than a cosmetic flaw. Same rule as the
        // wheel: where two mechanisms can serve one input, leave exactly one of them holding it.
        if pressed & SAVE_PICKER_DRIVE_STRIP_LBUTTON_MASK != 0 {
            let Some(hit_row) = save_picker_stage_cursor()
                .and_then(|(_, stage_y)| save_picker_row_from_stage_y(stage_y))
            else {
                append_autoload_debug(format_args!(
                    "save-picker: left click ignored, pointer is over no list row native_cursor={cursor}"
                ));
                return;
            };
            let Ok(target) = i32::try_from(hit_row) else {
                return;
            };
            if save_picker_model_row_from_native_cursor(target).is_none() {
                return;
            }
            let mut moved = false;
            if target != cursor
                && let (Ok(index), Ok(select)) = (
                    u32::try_from(target),
                    game_rva(MENU_ITEM_LIST_SET_CURSOR_RVA as u32),
                )
            {
                let select: unsafe extern "system" fn(usize, u32) -> u64 =
                    unsafe { std::mem::transmute(select) };
                unsafe { select(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET, index) };
                moved = true;
            }
            append_autoload_debug(format_args!(
                "save-picker: left click selects row={hit_row} (was native_cursor={cursor} model_row={model_row}) moved={moved}; the game owns the activation"
            ));
            return;
        }
        append_autoload_debug(format_args!(
            "save-picker-nav: pressed_mask=0x{pressed:x} ignored native_cursor={cursor} model_row={model_row} drive_row={drive_row:?}"
        ));
        return;
    }

    #[derive(Clone, Copy)]
    enum DriveStripPumpAction {
        Cell { cell: usize, x: f32, y: f32 },
        CurrentPath { x: f32, y: f32 },
        Cycle { forward: bool },
    }

    let action = if pressed & SAVE_PICKER_DRIVE_STRIP_LBUTTON_MASK != 0 {
        let chosen = {
            let guard = crate::experiments::save_picker::active_save_picker_lock();
            let Some(model) = guard.as_ref() else {
                return;
            };
            save_picker_drive_strip_hit_from_live_cursor(model.drive_strip_cell_count())
        };
        let Some((hit, x, y)) = chosen else {
            append_autoload_debug(format_args!(
                "save-picker: drive-strip pump mouse ignored at native_cursor={cursor} model_row={model_row}; no drive/path control under stage cursor pressed_mask=0x{pressed:x}"
            ));
            return;
        };
        match hit {
            DriveStripPointerHit::Cell(cell) => DriveStripPumpAction::Cell { cell, x, y },
            DriveStripPointerHit::CurrentPath => DriveStripPumpAction::CurrentPath { x, y },
        }
    } else if pressed & SAVE_PICKER_DRIVE_STRIP_LEFT_MASK != 0 {
        DriveStripPumpAction::Cycle { forward: false }
    } else if pressed & SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK != 0 {
        DriveStripPumpAction::Cycle { forward: true }
    } else {
        return;
    };

    let changed = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_mut() else {
            return;
        };
        match action {
            DriveStripPumpAction::Cell { cell, .. } => model.activate_drive_strip_cell(cell),
            DriveStripPumpAction::CurrentPath { .. } => model.focus_current_path_from_drive_strip(),
            DriveStripPumpAction::Cycle { forward } => model.cycle_drive_from_drive_strip(forward),
        }
    };

    match action {
        DriveStripPumpAction::Cell { cell, x, y } => append_autoload_debug(format_args!(
            "save-picker: drive-strip pump mouse native_cursor={cursor} model_row={model_row} stage_x={x:.1} stage_y={y:.1} cell={cell} changed={changed}"
        )),
        DriveStripPumpAction::CurrentPath { x, y } => append_autoload_debug(format_args!(
            "save-picker-path: path control focused by mouse native_cursor={cursor} model_row={model_row} stage_x={x:.1} stage_y={y:.1} changed={changed}"
        )),
        DriveStripPumpAction::Cycle { forward } => append_autoload_debug(format_args!(
            "save-picker: drive-strip pump key native_cursor={cursor} model_row={model_row} direction={} changed={changed}",
            if forward { "right" } else { "left" }
        )),
    }
    if !changed {
        return;
    }
    let staged = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        match guard.as_ref() {
            Some(model) => unsafe { save_picker_stage_row_records(model) },
            None => false,
        }
    };
    if staged {
        append_autoload_debug(format_args!(
            "save-picker: drive-strip pump restaged browse rows at native_cursor={cursor} model_row={model_row} pressed_mask=0x{pressed:x}"
        ));
        unsafe { save_picker_rebuild_profile_dialog_now(dialog, "drive-strip-pump") };
    }
}

fn save_picker_scrollbar_packed_state(current: usize, page: usize, total: usize) -> usize {
    (current.min(0xffff) & 0xffff)
        | ((page.min(0xffff) & 0xffff) << 16)
        | ((total.min(0xffff) & 0xffff) << 32)
}

/// Menu-pump-owned native scrollbar maintenance. The compact picker still stages only ten
/// `ProfileSummary` rows, so do not change the native GridControl item count here: that would let
/// native cursor movement address unstaged rows. Instead, drive the embedded native `ScrollBarV`
/// controller directly through the same total/current setters the game uses, with the verified
/// owner pointer at `ProfileLoadDialog + 0xbe0` (`grid + 0x1a8`).
pub(crate) unsafe fn save_picker_menu_pump_native_scrollbar() {
    let window = save_picker_live_profile_dialog();
    if window == 0 || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 {
        SAVE_PICKER_SCROLLBAR_LAST_SYNC.store(usize::MAX, Ordering::SeqCst);
        return;
    }

    let (current, page, total) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return;
        };
        let page = model.entries_per_page().max(1);
        let total = model.entry_count().max(page);
        (
            model.scroll_offset().min(total.saturating_sub(page)),
            page,
            total,
        )
    };

    let Ok(set_total_addr) = game_rva(SCROLLBAR_CONTROL_SET_TOTAL_RVA) else {
        return;
    };
    let Ok(set_position_addr) = game_rva(SCROLLBAR_CONTROL_SET_POSITION_RVA) else {
        return;
    };
    let scrollbar = window + PROFILE_LOAD_DIALOG_SCROLLBAR_OFFSET;
    // Both setters call FUN_14074dcc0, which immediately executes
    // `call [*(scrollbar+8)+8]` to toggle the embedded visible component. ProfileSelect's native
    // in-place rebuild can leave that component unbound between teardown/rebind frames. Calling the
    // setter then jumps through NULL (observed crash: rcx=dialog+0xbe8, ret=game+0x73334f).
    let Ok(base) = game_module_base() else {
        return;
    };
    let visible_value = unsafe { safe_read_usize(scrollbar + 8) }.unwrap_or(0);
    let set_visible_target = if visible_value != 0 {
        unsafe { safe_read_usize(visible_value + 8) }.unwrap_or(0)
    } else {
        0
    };
    if set_visible_target == 0 || !vtable_in_game_image(set_visible_target, base) {
        let skips = SAVE_PICKER_SCROLLBAR_DEAD_PROXY_SKIPS.fetch_add(1, Ordering::SeqCst) + 1;
        if skips <= 8 || skips.is_power_of_two() {
            append_autoload_debug(format_args!(
                "save-picker: native scrollbar sync skipped dead/unbound visible proxy #{skips} dialog=0x{window:x} scrollbar=0x{scrollbar:x} value=0x{visible_value:x} set_visible=0x{set_visible_target:x}"
            ));
        }
        SAVE_PICKER_SCROLLBAR_LAST_SYNC.store(usize::MAX, Ordering::SeqCst);
        return;
    }
    let set_total: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(set_total_addr) };
    let set_position: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(set_position_addr) };

    unsafe { set_total(scrollbar, total.min(i32::MAX as usize) as i32) };
    unsafe { set_position(scrollbar, current.min(i32::MAX as usize) as i32) };

    let packed = save_picker_scrollbar_packed_state(current, page, total);
    if SAVE_PICKER_SCROLLBAR_LAST_SYNC.swap(packed, Ordering::SeqCst) != packed {
        append_autoload_debug(format_args!(
            "save-picker: native scrollbar sync current={current} page={page} total={total} scrollbar=0x{scrollbar:x}"
        ));
    }
}

/// Learned `CSMenuManImp+0x90` event ids for vertical menu movement. `MoveA`(0x00) and `MoveB`(0x45)
/// are the two ids the vertical-move predicate reads, but which one is UP and which is DOWN is not
/// recorded anywhere -- so they are learned live, from a tick where exactly one id is set and
/// exactly one direction is pressed on a device. `MENU_EVENT_ID_UNLEARNED` until then.
static SAVE_PICKER_MENU_EVENT_DOWN_ID: AtomicUsize = AtomicUsize::new(MENU_EVENT_ID_UNLEARNED);
static SAVE_PICKER_MENU_EVENT_UP_ID: AtomicUsize = AtomicUsize::new(MENU_EVENT_ID_UNLEARNED);
const MENU_EVENT_ID_UNLEARNED: usize = usize::MAX;

/// A press deferred at an extreme row, waiting for the native wrap it is about to cause, and how
/// many pump ticks it may wait. Four ticks is generous for a wrap the list performs on the very
/// next frame, and short enough that an unredeemed press cannot resurface as a phantom step later.
static SAVE_PICKER_PENDING_WRAP_MASK: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_PENDING_WRAP_TICKS: AtomicUsize = AtomicUsize::new(0);
const PENDING_WRAP_MAX_TICKS: usize = 4;

/// Vertical menu events dropped at a listing limit; diagnostic only.
static SAVE_PICKER_LIMIT_SUPPRESSED_EVENTS: AtomicUsize = AtomicUsize::new(0);
/// Selection moves with no key/pad/wheel behind them, i.e. the pointer; diagnostic only.
static SAVE_PICKER_POINTER_CURSOR_MOVES: AtomicUsize = AtomicUsize::new(0);
/// Times the grid scrolled its own view during a select and had to be put back.
#[allow(dead_code)] // Retained: Picker diagnostic counter, beside the sibling counters that are live.
static SAVE_PICKER_GRID_VIEW_RESTORES: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_GRID_GEOMETRY_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// The live `CSMenuManImp` keystate bitmap (`+0x90`), one byte per menu event id.
unsafe fn save_picker_menu_event_keystate() -> Option<*mut u8> {
    let base = game_module_base().ok()?;
    let inputmgr = unsafe { *((base + CS_MENU_MAN_GLOBAL_RVA) as *const usize) };
    (inputmgr != 0).then(|| (inputmgr + INPUTMGR_BITMAP_90_OFFSET) as *mut u8)
}

/// Learn which vertical event id means DOWN and which means UP, from an unambiguous frame.
///
/// Ambiguous frames are skipped rather than guessed: getting this backwards would suppress the
/// direction that still has somewhere to go, which is worse than not suppressing at all.
unsafe fn save_picker_learn_vertical_menu_event_ids(down: bool, up: bool) {
    if down == up
        || SAVE_PICKER_MENU_EVENT_DOWN_ID.load(Ordering::SeqCst) != MENU_EVENT_ID_UNLEARNED
    {
        return;
    }
    let Some(keystate) = (unsafe { save_picker_menu_event_keystate() }) else {
        return;
    };
    let a_set = unsafe { *keystate.add(MENU_EVENT_MOVE_A_00) } & MENU_EVENT_PRESSED_BIT != 0;
    let b_set = unsafe { *keystate.add(MENU_EVENT_MOVE_B_45) } & MENU_EVENT_PRESSED_BIT != 0;
    if a_set == b_set {
        return;
    }
    let pressed_id = if a_set {
        MENU_EVENT_MOVE_A_00
    } else {
        MENU_EVENT_MOVE_B_45
    };
    let other_id = if a_set {
        MENU_EVENT_MOVE_B_45
    } else {
        MENU_EVENT_MOVE_A_00
    };
    let (down_id, up_id) = if down {
        (pressed_id, other_id)
    } else {
        (other_id, pressed_id)
    };
    SAVE_PICKER_MENU_EVENT_UP_ID.store(up_id, Ordering::SeqCst);
    SAVE_PICKER_MENU_EVENT_DOWN_ID.store(down_id, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker: learned vertical menu event ids down=0x{down_id:x} up=0x{up_id:x}"
    ));
}

static SAVE_PICKER_SET_CURSOR_ORIG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_SET_CURSOR_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_SET_CURSOR_NEUTRALISED: AtomicUsize = AtomicUsize::new(0);
/// Wheel detents the native grid refused (view base at a clamp) that this pump stepped instead.
static SAVE_PICKER_WHEEL_NATIVE_STEPS: AtomicUsize = AtomicUsize::new(0);

/// `FUN_14073bc10` detour: neutralise the ensure-visible base for EVERY select on the picker's list.
///
/// The wheel step can zero the base around its own call, but the game makes this call itself on
/// every mouse hover and click, and those resets are what re-orient the list under a stationary
/// pointer. Hooking is the only place to reach them: the base and the index are in different spaces
/// (scrollbar model-space vs view-space 0..9) and only this function compares the two.
pub(crate) unsafe extern "system" fn save_picker_set_cursor_hook(list: usize, index: u32) -> u64 {
    let orig_addr = SAVE_PICKER_SET_CURSOR_ORIG.load(Ordering::SeqCst);
    if orig_addr == 0 {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, u32) -> u64 =
        unsafe { std::mem::transmute(orig_addr) };
    // Only the picker's own list, and only while the picker owns the screen: every other menu in
    // the game uses this grid the way it was designed and must keep its native scrolling.
    let dialog = save_picker_live_profile_dialog();
    let ours = dialog != 0
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
        && list == dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET;
    if !ours {
        return unsafe { orig(list, index) };
    }
    let before = unsafe { save_picker_grid_view_base(list) };
    if before != (0, 0) {
        unsafe { save_picker_set_grid_view_base(list, (0, 0)) };
    }
    let ret = unsafe { orig(list, index) };
    let after = unsafe { save_picker_grid_view_base(list) };
    if after != before {
        unsafe { save_picker_set_grid_view_base(list, before) };
        let n = SAVE_PICKER_SET_CURSOR_NEUTRALISED.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 20 || n.is_multiple_of(50) {
            append_autoload_debug(format_args!(
                "save-picker: native select neutralised #{n} index={index} view {before:?} (call left {after:?})"
            ));
        }
    }
    ret
}

pub(crate) fn install_save_picker_set_cursor_hook() {
    if SAVE_PICKER_SET_CURSOR_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Ok(addr) = game_rva(MENU_ITEM_LIST_SET_CURSOR_RVA as u32) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve select-index rva 0x{MENU_ITEM_LIST_SET_CURSOR_RVA:x}"
        ));
        SAVE_PICKER_SET_CURSOR_HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_set_cursor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_SET_CURSOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable select-index failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked list select-index FUN_14073bc10 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: select-index MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new select-index failed: {status:?}"
        )),
    }
}

/// `FUN_140757c70` -- the ONLY place the grid reads a wheel notch. Byte-verified unique in the
/// 1.16.2 deobf image at `0x140757c70` (`48 89 5c 24 08 57 48 83 ec 20 48 8b da 48 8b f9 ba 2c ..`).
///
/// It resolves the wheel to a `(col, row)` step from menu event ids `0x2c` (up, row -1) and `0x2d`
/// (down, row +1) via `FUN_14075d8f0`, and its only two callers are the grid mouse handler
/// `FUN_14073a5c0` and `FUN_140781460`.
const MENU_EVENT_WHEEL_DELTA_ACCESSOR_RVA: usize = 0x757c70;
static SAVE_PICKER_WHEEL_DELTA_ORIG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_WHEEL_DELTA_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_WHEEL_DELTA_SILENCED: AtomicUsize = AtomicUsize::new(0);

/// THE INTERLOCK: while the picker owns the screen, the game's own grid never sees a wheel notch.
///
/// Two mechanisms can scroll this list for one detent -- the native grid handler and this pump --
/// and the double scroll is simply both of them running. Every attempt to arbitrate them by TIMING
/// failed, and the live log says why: the handler acts LATER than the tick the detent arrives on and
/// later than the tick after it too (our step at `+107884ms`, the handler's move only visible at
/// `+107911ms`), so there is no tick on which the pump can ask "did the game already take this one?"
/// and get a true answer. Deferring by a fixed number of ticks just moves the guess.
///
/// So do not arbitrate: remove one of the two mechanisms. Zeroing the delta here makes the wheel
/// branch in `FUN_14073a5c0` (`if (delta.col != 0 || delta.row != 0)`) fall through, so the native
/// grid performs no view scroll and no cursor move at all, and the pump is the sole owner of the
/// wheel with no timing assumption anywhere. It also removes the reason the wheel was uneven in the
/// first place: the native step was gated on the grid's own view base being able to move, which is
/// false at a clamp, so the game was an unreliable owner even when it was the only one.
///
/// Scoped to the picker's own screen, and it silences a READ rather than dropping the user's input:
/// our own wheel latch comes from `GetRawInputData` and is untouched, so the detent still reaches
/// the picker. Every other menu keeps its native wheel exactly as designed.
unsafe extern "system" fn save_picker_wheel_delta_hook(msg: usize, out: *mut i32) -> *mut i32 {
    let orig_addr = SAVE_PICKER_WHEEL_DELTA_ORIG.load(Ordering::SeqCst);
    if orig_addr == 0 {
        return out;
    }
    let orig: unsafe extern "system" fn(usize, *mut i32) -> *mut i32 =
        unsafe { std::mem::transmute(orig_addr) };
    let ret = unsafe { orig(msg, out) };
    let owned = save_picker_live_profile_dialog() != 0
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0;
    if !owned || out.is_null() {
        return ret;
    }
    let had_notch = unsafe { out.read_unaligned() != 0 || out.add(1).read_unaligned() != 0 };
    if had_notch {
        unsafe {
            out.write_unaligned(0);
            out.add(1).write_unaligned(0);
        }
        let n = SAVE_PICKER_WHEEL_DELTA_SILENCED.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 20 || n.is_multiple_of(50) {
            append_autoload_debug(format_args!(
                "save-picker: silenced native wheel notch #{n} (the pump owns the wheel)"
            ));
        }
    }
    ret
}

pub(crate) fn install_save_picker_wheel_delta_hook() {
    if SAVE_PICKER_WHEEL_DELTA_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Ok(addr) = game_rva(MENU_EVENT_WHEEL_DELTA_ACCESSOR_RVA as u32) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve wheel-delta rva 0x{MENU_EVENT_WHEEL_DELTA_ACCESSOR_RVA:x}"
        ));
        SAVE_PICKER_WHEEL_DELTA_HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_wheel_delta_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_WHEEL_DELTA_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable wheel-delta failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked wheel-delta FUN_140757c70 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: wheel-delta MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new wheel-delta failed: {status:?}"
        )),
    }
}

/// Move the picker's selection one row for a wheel detent the native grid declined to act on.
///
/// This calls `FUN_14073bc10` -- the list's own select-index primitive, the same call the grid's
/// mouse hit test makes (`FUN_14073a5c0` tail) and the same one the wheel path would have reached
/// via `FUN_14073b0c0` had its view-base gate let it through. Going through the select rather than
/// writing `list+0xd4` is what carries the chrome with the selection; a bare field write moves the
/// index and leaves the highlight where it was, which is the "rows scroll but the chrome doesn't
/// travel" half of the report. The call re-enters our own detour above, so the view base stays
/// pinned exactly as it does for a hover or a click.
unsafe fn save_picker_wheel_step_native_cursor(
    dialog: usize,
    model_row: usize,
    from_cursor: i32,
) -> i32 {
    let Ok(index) = i32::try_from(model_row)
        .map(|row| row.saturating_add(PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET))
        .and_then(u32::try_from)
    else {
        return from_cursor;
    };
    let Ok(select) = game_rva(MENU_ITEM_LIST_SET_CURSOR_RVA as u32) else {
        return from_cursor;
    };
    let select: unsafe extern "system" fn(usize, u32) -> u64 =
        unsafe { std::mem::transmute(select) };
    let ret = unsafe { select(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET, index) };
    // Keep the pump's edge sampling honest: the next tick compares against this, and leaving the
    // pre-step row here would read our own step back as a NATIVE move and swallow the next detent.
    SAVE_PICKER_EDGE_SCROLL_PREV_CURSOR.store(
        usize::try_from(index).unwrap_or(EDGE_SCROLL_NO_PREV_CURSOR),
        Ordering::SeqCst,
    );
    let n = SAVE_PICKER_WHEEL_NATIVE_STEPS.fetch_add(1, Ordering::SeqCst) + 1;
    if n <= 20 || n.is_multiple_of(25) {
        append_autoload_debug(format_args!(
            "save-picker: wheel step #{n} the grid declined from={from_cursor} to_index={index} select_ret={ret}"
        ));
    }
    i32::try_from(index).unwrap_or(from_cursor)
}

/// `FUN_140736c90(grid, point)` -- the grid's pointer hit test, byte-verified unique at
/// `0x140736c90` in the 1.16.2 deobf image.
const MENU_ITEM_LIST_POINT_TO_INDEX_RVA: usize = 0x736c90;
static SAVE_PICKER_HIT_TEST_ORIG: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_HIT_TEST_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_HIT_TEST_REBASED: AtomicUsize = AtomicUsize::new(0);

/// Neutralise the view base for the grid's POINTER HIT TEST, the same way the select hook does for
/// the select itself.
///
/// The hit test walks the visible cells and turns the one under the pointer into an ABSOLUTE item
/// index by adding the view base, then discards the hit if that index is past the item count:
///
///     140736d41  MOV  R11D, [RSI + 0x348]   ; view row base
///     140736d80  LEA  EDI, [R10 + R11*1]    ; view row + base
///     140736daf  CMP  [RSI + 0xd0], EDI     ; count vs index
///     140736db5  JLE  ...                   ; index >= count -> report NO hit
///
/// The picker keeps its MODEL's scroll offset in that base so the native scrollbar thumb tracks a
/// listing far longer than the ten staged records (`save-picker: native scrollbar sync`). For the
/// hit test that offset is poison: with base 10 against 10 records every visible cell computes an
/// index >= count, so the pointer hits nothing, nothing is selected, and the game's click
/// activation has nothing to act on. Clicking therefore worked only while the scrollbar sat at the
/// very top, where the base happens to be 0 -- reported 2026-08-12, and the same shape as the wheel
/// dying at a clamped base.
///
/// Zeroing the base for the duration of the call makes the hit test return a VIEW-relative index
/// `0..9`, which is exactly the space the ten staged records live in and the space the select hook
/// already leaves `+0xd4` in. The base is restored immediately afterwards, so the scrollbar thumb is
/// unaffected.
unsafe extern "system" fn save_picker_hit_test_hook(list: usize, point: usize) -> u32 {
    let orig_addr = SAVE_PICKER_HIT_TEST_ORIG.load(Ordering::SeqCst);
    if orig_addr == 0 {
        return u32::MAX;
    }
    let orig: unsafe extern "system" fn(usize, usize) -> u32 =
        unsafe { std::mem::transmute(orig_addr) };
    let dialog = save_picker_live_profile_dialog();
    let ours = dialog != 0
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
        && list == dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET;
    if !ours {
        return unsafe { orig(list, point) };
    }
    let before = unsafe { save_picker_grid_view_base(list) };
    if before == (0, 0) {
        return unsafe { orig(list, point) };
    }
    unsafe { save_picker_set_grid_view_base(list, (0, 0)) };
    let ret = unsafe { orig(list, point) };
    unsafe { save_picker_set_grid_view_base(list, before) };
    let n = SAVE_PICKER_HIT_TEST_REBASED.fetch_add(1, Ordering::SeqCst) + 1;
    if n <= 20 || n.is_multiple_of(100) {
        append_autoload_debug(format_args!(
            "save-picker: hit test rebased #{n} view {before:?} -> (0, 0) index={ret}"
        ));
    }
    ret
}

pub(crate) fn install_save_picker_hit_test_hook() {
    if SAVE_PICKER_HIT_TEST_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Ok(addr) = game_rva(MENU_ITEM_LIST_POINT_TO_INDEX_RVA as u32) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve hit-test rva 0x{MENU_ITEM_LIST_POINT_TO_INDEX_RVA:x}"
        ));
        SAVE_PICKER_HIT_TEST_HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_hit_test_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_HIT_TEST_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable hit-test failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked pointer hit test FUN_140736c90 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: hit-test MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new hit-test failed: {status:?}"
        )),
    }
}

/// The grid's own view-scroll base as `(column, row)`.
unsafe fn save_picker_grid_view_base(list: usize) -> (i32, i32) {
    unsafe {
        (
            *((list + GRID_CONTROL_VIEW_COL_BASE_OFFSET) as *const i32),
            *((list + GRID_CONTROL_VIEW_ROW_BASE_OFFSET) as *const i32),
        )
    }
}

unsafe fn save_picker_set_grid_view_base(list: usize, base: (i32, i32)) {
    unsafe {
        *((list + GRID_CONTROL_VIEW_COL_BASE_OFFSET) as *mut i32) = base.0;
        *((list + GRID_CONTROL_VIEW_ROW_BASE_OFFSET) as *mut i32) = base.1;
    }
}

/// Log the grid's index space once per picker session: the select call bounds-checks against these,
/// and whether the cursor index is absolute or view-relative depends on them.
unsafe fn save_picker_log_grid_geometry_once(list: usize) {
    if SAVE_PICKER_GRID_GEOMETRY_LOGGED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let (count, cols, rows) = unsafe {
        (
            *((list + GRID_CONTROL_ITEM_COUNT_OFFSET) as *const i32),
            *((list + GRID_CONTROL_COLUMNS_OFFSET) as *const i32),
            *((list + GRID_CONTROL_ROWS_OFFSET) as *const i32),
        )
    };
    let view = unsafe { save_picker_grid_view_base(list) };
    append_autoload_debug(format_args!(
        "save-picker: grid geometry count={count} cols={cols} rows={rows} view_base={view:?}"
    ));
}

/// Clear this frame's vertical menu event so the native list never moves.
///
/// The list animates its own cursor move the instant it consumes the event, so a correction written
/// afterwards still lets the animation play -- which is what a player sees at the end of a listing
/// as a scroll that "happens" and then undoes itself. This runs from the MenuWindowJob::Run POST
/// hook: `Run` is the producer that sets `+0x90[id] |= 1`, and the menu's own Update consumes it
/// later in the frame, so clearing here lands between the two.
unsafe fn save_picker_clear_vertical_menu_event(down: bool) -> bool {
    let id = if down {
        SAVE_PICKER_MENU_EVENT_DOWN_ID.load(Ordering::SeqCst)
    } else {
        SAVE_PICKER_MENU_EVENT_UP_ID.load(Ordering::SeqCst)
    };
    if id == MENU_EVENT_ID_UNLEARNED {
        return false;
    }
    let Some(keystate) = (unsafe { save_picker_menu_event_keystate() }) else {
        return false;
    };
    let byte = unsafe { keystate.add(id) };
    if unsafe { *byte } & MENU_EVENT_PRESSED_BIT == 0 {
        return false;
    }
    unsafe { *byte &= !MENU_EVENT_PRESSED_BIT };
    true
}

/// Menu-pump-owned scroll-window maintenance. The native ProfileSelect backing list has only ten
/// row models, so long directory listings are represented as a sliding ten-row window with no page
/// or pseudo-scroll rows. When the native cursor rests on a window edge, the model advances and this
/// queues the same in-place rebuild used by directory/drive navigation.
/// Scroll the ten-row native window by one row per explicit UP/DOWN press taken at an edge row.
/// Consuming the latch unconditionally (even when the picker is not live) keeps a press made
/// elsewhere from being replayed into the list the next time the picker opens.
pub(crate) unsafe fn save_picker_menu_pump_edge_scroll() {
    let up_mask = crate::experiments::SAVE_PICKER_NAV_UP_MASK;
    let down_mask = crate::experiments::SAVE_PICKER_NAV_DOWN_MASK;
    let wheel_up_mask = crate::experiments::SAVE_PICKER_NAV_WHEEL_UP_MASK;
    let wheel_down_mask = crate::experiments::SAVE_PICKER_NAV_WHEEL_DOWN_MASK;
    let nav_edges = crate::experiments::save_picker_take_user_nav_edges_for(
        up_mask | down_mask | wheel_up_mask | wheel_down_mask,
    );
    let wheel_down = nav_edges & wheel_down_mask != 0;
    let wheel_up = nav_edges & wheel_up_mask != 0;
    let held = crate::experiments::save_picker_user_nav_held();
    let dialog = save_picker_live_profile_dialog();
    if dialog == 0 || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 {
        SAVE_PICKER_EDGE_SCROLL_PREV_CURSOR.store(EDGE_SCROLL_NO_PREV_CURSOR, Ordering::SeqCst);
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let cursor_getter: unsafe extern "system" fn(usize) -> i32 =
        unsafe { std::mem::transmute(base + MENU_ITEM_LIST_CURSOR_GETTER_RVA) };
    let cursor = unsafe { cursor_getter(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET) };
    unsafe { save_picker_log_grid_geometry_once(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET) };
    // Remember where the selection was BEFORE this tick's key was read. The native list moves and
    // wraps its own cursor the moment it sees the press, so by the time this pump runs the sampled
    // row is already the wrap destination -- at the bottom row a DOWN press reads back as the
    // drives row. Judging the edge on the post-press row is why holding DOWN stopped scrolling.
    let prev_cursor = SAVE_PICKER_EDGE_SCROLL_PREV_CURSOR.swap(
        usize::try_from(cursor).unwrap_or(EDGE_SCROLL_NO_PREV_CURSOR),
        Ordering::SeqCst,
    );
    let (last_visible_row, first_content_row, at_scroll_top, at_scroll_bottom) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return;
        };
        (
            model.visible_row_count().saturating_sub(1),
            model.entry_row_base(),
            model.scroll_offset() == 0,
            model.scroll_offset() >= model.scroll_max(),
        )
    };
    let edge_down = nav_edges & down_mask != 0;
    let edge_up = nav_edges & up_mask != 0;
    unsafe {
        save_picker_learn_vertical_menu_event_ids(
            edge_down || held & down_mask != 0,
            edge_up || held & up_mask != 0,
        )
    };
    // SUPPRESS AT A HARD LIMIT, every tick rather than only when an edge was latched. The listing
    // has nothing further that way, so the native list must not move at all -- not move-and-be-
    // corrected, which is the same pixels animating for a change that never happens. Checked from
    // the cursor's CURRENT row so the very first press is caught, not just the repeats after it.
    let blocked = if cursor >= 0 && usize::try_from(cursor).is_ok_and(|c| c >= last_visible_row) {
        at_scroll_bottom.then_some(true)
    } else if cursor == 0 {
        at_scroll_top.then_some(false)
    } else {
        None
    };
    if let Some(blocked_down) = blocked
        && unsafe { save_picker_clear_vertical_menu_event(blocked_down) }
    {
        let n = SAVE_PICKER_LIMIT_SUPPRESSED_EVENTS.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 || n.is_multiple_of(50) {
            append_autoload_debug(format_args!(
                "save-picker: suppressed vertical menu event #{n} at listing limit down={blocked_down} cursor={cursor} last_visible_row={last_visible_row}"
            ));
        }
        SAVE_PICKER_EDGE_SCROLL_PREV_CURSOR.store(
            usize::try_from(cursor).unwrap_or(EDGE_SCROLL_NO_PREV_CURSOR),
            Ordering::SeqCst,
        );
        return;
    }
    // NATIVE SELECTION MOVE: the cursor changed with no latched input of ours behind it. Named for
    // what it measures rather than a guessed cause -- reading it as "the pointer" is how a wheel
    // detent's NATIVE step got mistaken for mouse movement, and a duplicate step shipped on top of
    // it. The model's scroll state rides along because a report of rows re-orienting cannot be
    // attributed without knowing whether OUR window moved or the game re-laid itself out.
    if nav_edges == 0
        && !wheel_down
        && !wheel_up
        && held == 0
        && prev_cursor != EDGE_SCROLL_NO_PREV_CURSOR
        && prev_cursor != usize::try_from(cursor).unwrap_or(EDGE_SCROLL_NO_PREV_CURSOR)
    {
        let n = SAVE_PICKER_POINTER_CURSOR_MOVES.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 40 || n.is_multiple_of(25) {
            let (offset, max) = {
                let guard = crate::experiments::save_picker::active_save_picker_lock();
                guard
                    .as_ref()
                    .map(|model| (model.scroll_offset(), model.scroll_max()))
                    .unwrap_or((usize::MAX, usize::MAX))
            };
            append_autoload_debug(format_args!(
                "save-picker: native selection move #{n} {prev_cursor} -> {cursor} scroll_offset={offset}/{max} last_visible_row={last_visible_row}"
            ));
        }
    }
    // Did the native list just WRAP? A wrap is a jump between the two extreme rows in one sample,
    // and it is the only motion the list makes that the player never asked for. Detecting the wrap
    // itself -- rather than only the key edge that caused the first one -- is what covers a HELD
    // direction: the menu auto-repeats and each repeat moves the cursor, but only the first press
    // ever produces an edge, so an edge-only rule guards one step and lets every later repeat wrap.
    let wrapped_from = (prev_cursor != EDGE_SCROLL_NO_PREV_CURSOR)
        .then(|| {
            let prev = i64::try_from(prev_cursor).unwrap_or(-1);
            let now = i64::from(cursor);
            let last = i64::try_from(last_visible_row).unwrap_or(0);
            if last <= 0 {
                None
            } else if prev == last && now == 0 {
                Some(down_mask)
            } else if prev == 0 && now == last {
                Some(up_mask)
            } else {
                None
            }
        })
        .flatten();
    // Age out a deferred press whose wrap never arrived, so it cannot be redeemed much later.
    let pending = SAVE_PICKER_PENDING_WRAP_MASK.load(Ordering::SeqCst);
    if pending != 0 && SAVE_PICKER_PENDING_WRAP_TICKS.fetch_sub(1, Ordering::SeqCst) <= 1 {
        SAVE_PICKER_PENDING_WRAP_MASK.store(0, Ordering::SeqCst);
        SAVE_PICKER_PENDING_WRAP_TICKS.store(0, Ordering::SeqCst);
    }
    // A wrap only counts as navigation when that direction is actually held on a device, or when a
    // press we deliberately deferred is still owed its step. Without that check a fast MOUSE sweep
    // across the list -- hover writes the same cursor field -- would read as a wrap and get yanked
    // back under the pointer.
    let wrap_nav = wrapped_from
        .filter(|mask| held & mask != 0 || pending & mask != 0)
        .unwrap_or(0);
    // ONE PRESS, ONE STEP. At an extreme row the native list ALWAYS wraps, so the key edge and the
    // wrap it causes are the SAME press seen one tick apart. Acting on both scrolled the window
    // twice and rebuilt the list twice for a single press -- visible in the live log as two
    // `reason=edge-scroll-pump` rebuilds 29ms apart, and on screen as the list running away faster
    // than the presses. Defer to the wrap at an extreme row; act on the edge only where no wrap can
    // follow. The pending latch carries the press across the gap so a tap released before the wrap
    // is sampled still gets its step.
    // A WHEEL detent is exempt from that deferral: the native list does not wrap for the wheel, so
    // there is no wrap coming to defer to. Deferring one anyway is why the wheel did nothing at
    // exactly the top and bottom rows -- the only rows where the wheel has to do the work itself.
    let wheel_nav = wheel_down || wheel_up;
    let edge_at_extreme = (edge_down && prev_cursor == last_visible_row)
        || (edge_up && prev_cursor == 0 && last_visible_row > 0);
    if wrap_nav == 0 && edge_at_extreme && !wheel_nav {
        SAVE_PICKER_PENDING_WRAP_MASK.store(
            if edge_down { down_mask } else { up_mask },
            Ordering::SeqCst,
        );
        SAVE_PICKER_PENDING_WRAP_TICKS.store(PENDING_WRAP_MAX_TICKS, Ordering::SeqCst);
        return;
    }
    if wrap_nav != 0 {
        SAVE_PICKER_PENDING_WRAP_MASK.store(0, Ordering::SeqCst);
        SAVE_PICKER_PENDING_WRAP_TICKS.store(0, Ordering::SeqCst);
    }
    let down = ((nav_edges | wrap_nav) & down_mask != 0) || wheel_down;
    let up = ((nav_edges | wrap_nav) & up_mask != 0) || wheel_up;
    // A simultaneous up+down edge has no direction; ignore rather than pick one arbitrarily.
    if up == down {
        return;
    }
    // Key and pad steps are judged from the row held BEFORE the press, because the native list has
    // already moved its cursor by the time this runs. A wheel detent is sampled from the CURRENT
    // row instead: the native list handles the wheel within the same tick rather than a tick later,
    // and a one-tick-old sample would misjudge the edge after any mouse movement, since hover writes
    // the same cursor field.
    let wheel_only = wheel_nav && (nav_edges & (up_mask | down_mask)) == 0 && wrap_nav == 0;
    let press_row = if wheel_only || prev_cursor == EDGE_SCROLL_NO_PREV_CURSOR {
        cursor
    } else {
        i32::try_from(prev_cursor).unwrap_or(cursor)
    };
    let Some(model_row) = save_picker_model_row_from_native_cursor(press_row) else {
        return;
    };
    let outcome = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_mut() else {
            return;
        };
        model.scroll_window_from_edge_press(model_row, down)
    };
    let Some(outcome) = outcome else {
        // Away from an edge there is normally no window work AND no cursor work: the native list
        // moves its own selection for a key, a pad direction AND a wheel detent.
        //
        // The WHEEL is the exception, and the reason is in the grid's own mouse handler. In
        // `FUN_14073a5c0` the wheel branch reads the notch delta and then, per notch:
        //
        //     if (FUN_14073b670(grid, delta))                  // scroll the VIEW; "did base move?"
        //         FUN_14073b0c0(grid, grid->cursor, delta);    // ...only then move the CURSOR
        //
        // so the detent's cursor step is GATED on the grid's own view base (`grid+0x348`, the
        // scrollbar position) actually changing. `FUN_14073b670` clamps the new base into
        // `[0, ((count-1)/cols) - rows + 1]` (`FUN_14073a0a0`) and reports "unchanged" at either
        // clamp -- and the picker owns scrolling in its MODEL, staging a fixed window of records, so
        // that native range is a degenerate one or two positions that our select hook then pins.
        // At a clamped base the whole detent reaches nothing at all: no view move, no cursor move,
        // no chrome. That is the reported dead wheel at the top of the scrollbar.
        //
        // The wheel is UNCONDITIONALLY ours, and it is safe to act on the spot only because the
        // other mechanism no longer exists: `save_picker_wheel_delta_hook` zeroes the notch the grid
        // would have read, so `FUN_14073a5c0` never scrolls or moves the cursor while the picker is
        // up. Two earlier shapes of this both double-scrolled, because both tried to decide WHO acts
        // by looking at the cursor -- once on the arrival tick, once a tick later -- and the native
        // handler runs later than either (live log: our step `+107884ms`, its move `+107911ms`).
        // There is no tick that answers the question, so the question had to stop being asked.
        if wheel_only {
            let step_row = if down {
                model_row.saturating_add(1).min(last_visible_row)
            } else {
                model_row.saturating_sub(1).max(first_content_row)
            };
            if step_row != model_row {
                unsafe { save_picker_wheel_step_native_cursor(dialog, step_row, cursor) };
            }
        }
        return;
    };
    let pinned_row = outcome.pin_row();
    // Hold the native selection on the row the press acted from. The list cursor is a plain field
    // (`FUN_140739e20` is just `*(u32 *)(list + 0xd4)`), and the game's own GridControl mouse hit
    // writes it directly, so a write is the native mechanism rather than a poke around one. Without
    // this the selection leaves the edge after each press and the window stops advancing -- and at a
    // hard limit the native list has already wrapped the cursor to the far end of the listing, so
    // the same write is what keeps the selection from teleporting there.
    //
    // A WHEEL step is exempt: it never moved the native cursor, so there is nothing to hold, and
    // writing anyway drags the selection off whatever row the POINTER is over -- the mouse and the
    // wheel fighting each other for the same field, which reads as mouse row navigation being
    // broken.
    let native_pin = (!wheel_only)
        .then(|| i32::try_from(pinned_row).ok())
        .flatten()
        .and_then(|row| row.checked_add(PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET));
    if let Some(native_pin) = native_pin {
        unsafe {
            *((dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET + MENU_ITEM_LIST_CURSOR_FIELD_OFFSET)
                as *mut i32) = native_pin;
        }
        // The sample stored at the top of this function is where the native list PUT the cursor,
        // which is the wrap destination we just overrode. Held keys repeat on consecutive ticks, so
        // leaving that stale value here would make the next press judge its edge from a row the
        // selection never visibly occupied.
        SAVE_PICKER_EDGE_SCROLL_PREV_CURSOR.store(pinned_row, Ordering::SeqCst);
    }
    if !outcome.scrolled() {
        append_autoload_debug(format_args!(
            "save-picker: edge-press held at listing limit down={down} press_row={press_row} model_row={model_row} wrap_target={cursor} pinned_native_row={native_pin:?}"
        ));
        return;
    }
    let staged = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        match guard.as_ref() {
            Some(model) => unsafe { save_picker_stage_row_records(model) },
            None => false,
        }
    };
    if staged {
        append_autoload_debug(format_args!(
            "save-picker: edge-scroll restaged browse rows post_press_cursor={cursor} press_row={press_row} model_row={model_row} pinned_native_row={native_pin:?}"
        ));
        unsafe { save_picker_rebuild_profile_dialog_now(dialog, "edge-scroll-pump") };
    }
}

/// Menu-pump-owned in-place list rebuild (called from the MenuWindowJob::Run hook). Runs the
/// native records-changed rebuild queued by a picker navigation; falls back to close+resubmit
/// when the rebuild fn cannot be resolved.
pub(crate) unsafe fn save_picker_menu_pump_rebuild() {
    let dialog = SAVE_PICKER_REBUILD_PENDING_DIALOG.swap(0, Ordering::SeqCst);
    if dialog == 0 || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 {
        return;
    }
    unsafe { save_picker_rebuild_profile_dialog_now(dialog, "queued-navigation") };
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
            if reopen_as_picker {
                "picker page"
            } else {
                "slot view"
            }
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
pub(crate) fn save_picker_html_escape(text: &str) -> String {
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
pub(crate) fn save_picker_browse_html_utf16(text: &str) -> Vec<u16> {
    save_picker_browse_html_utf16_color(text, "#8f887a")
}

pub(crate) fn save_picker_error_html_utf16(text: &str) -> Vec<u16> {
    save_picker_browse_html_utf16_color(text, "#d8a052")
}

pub(crate) fn save_picker_browse_html_utf16_color(text: &str, color: &str) -> Vec<u16> {
    // Match the native ProfileSelect filename/timestamp fields; the asset gives ErStats a native-height box.
    save_picker_html_utf16_color_size(text, color, 24)
}

fn save_picker_html_utf16_color_size(text: &str, color: &str, font_height: i32) -> Vec<u16> {
    if text.is_empty() {
        return vec![0];
    }
    let mut s = String::from("<p align=\"left\"><font size=\"");
    s.push_str(&font_height.to_string());
    s.push_str("\" color=\"");
    s.push_str(color);
    s.push_str("\">");
    s.push_str(&save_picker_html_escape(text));
    s.push_str("</font></p>");
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

pub(crate) fn save_picker_set_visible_status(message: er_save_picker::PickerStatusMessage) {
    if let Some(model) = crate::experiments::save_picker::active_save_picker_lock().as_mut() {
        model.set_status_message(message);
    }
}

/// Character budget for the per-file character list fragment. This text is merged onto the single
/// inline `ErStats` row field beside the filename and timestamp, so it must stay short enough to read
/// as row detail instead of a wrapped second line.
pub(crate) const SAVE_PICKER_BROWSE_LINE_CHAR_BUDGET: usize = 34;

pub(crate) fn save_picker_drive_cell_html_utf16(text: &str) -> Vec<u16> {
    // The button frame already supplies the visual boundary. Keep the model's `>C:<` / `[S:]`
    // wrappers for the boot overlay and selection semantics, but do not render that punctuation
    // inside the compact native button -- it clips before the drive letter does.
    let (selected, display) = if let Some(inner) = text
        .strip_prefix('>')
        .and_then(|inner| inner.strip_suffix('<'))
    {
        (true, inner)
    } else if let Some(inner) = text
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        (false, inner)
    } else {
        (false, text)
    };
    let color = if selected { "#d8a052" } else { "#8f887a" };
    let font_height = profile_editor_field_font_height("DriveCell_0");
    save_picker_html_utf16_color_size(display, color, font_height)
}

/// CurrentPath control colours: the parchment tone the rest of the picker chrome uses, and the
/// warning gold reserved for an entry the picker refused.
const SAVE_PICKER_PATH_NORMAL_COLOR: &str = "#b8b1a2";
const SAVE_PICKER_PATH_INVALID_COLOR: &str = "#e8c34a";

pub(crate) fn save_picker_current_path_text(row: usize) -> Option<Vec<u16>> {
    if save_picker_path_editor_active() {
        return Some(vec![0]);
    }
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let model = guard.as_ref()?;
    if model.drive_row() != Some(row) {
        return Some(vec![0]);
    }
    // A rejected entry outranks the real folder: the user sees exactly what they typed, marked
    // invalid, until they correct it (any successful commit or navigation refreshes the listing,
    // which drops the rejected text and returns this control to the normal colour).
    let (text, color) = match model.rejected_path_text() {
        Some(rejected) => (rejected, SAVE_PICKER_PATH_INVALID_COLOR),
        None => (model.current_dir().to_str()?, SAVE_PICKER_PATH_NORMAL_COLOR),
    };
    let escaped = save_picker_html_escape(text);
    let font_height = profile_editor_field_font_height("CurrentPath");
    let html = format!(
        "<p align=\"left\"><font size=\"{font_height}\" color=\"{color}\">{escaped}</font></p>"
    );
    Some(html.encode_utf16().chain(core::iter::once(0)).collect())
}

pub(crate) fn save_picker_drive_cell_text(row: usize, cell: usize) -> Option<Vec<u16>> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let model = guard.as_ref()?;
    let text = model.drive_row_cell_label(row, cell).unwrap_or_default();
    Some(save_picker_drive_cell_html_utf16(&text))
}

/// The `ErStats` fragments for ProfileSelect row `row` while the browse picker owns the window.
/// The row-populate hook merges the two fragments into ONE inline field: file rows show active-slot
/// count plus character names/levels beside `ER0000.sl2`, while navigation/status rows show their
/// auxiliary copy beside the row label. Empty rows get blank fragments so neither leftover row text
/// nor per-slot attribute stats render as junk there. `None` when the picker does not own the rows
/// (the normal character-slot view keeps the attribute stats panel).
pub(crate) fn save_picker_browse_stats_lines(row: usize) -> Option<(Vec<u16>, Vec<u16>)> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let model = guard.as_ref()?;
    let status_row = model.status_message().is_some() && row == 0;
    if let Some((top, bottom)) = model.row_auxiliary_lines(row) {
        if status_row {
            return Some((
                save_picker_error_html_utf16(&top),
                save_picker_error_html_utf16(&bottom),
            ));
        }
        return Some((
            save_picker_browse_html_utf16(&top),
            save_picker_browse_html_utf16(&bottom),
        ));
    }
    let is_current = model.row_is_loaded_save(row);
    let Some(chars) = model.row_file_characters(row) else {
        // Empty row: blank the injected stats field so no per-slot attribute stats render as junk.
        return Some((vec![0], vec![0]));
    };
    let count = if chars.len() == 1 {
        "1 CHAR".to_owned()
    } else {
        format!("{} CHAR", chars.len())
    };
    let top = if is_current {
        format!("* {count}")
    } else {
        count
    };
    let mut bottom = String::new();
    let mut shown = 0usize;
    for info in chars {
        let seg = format!("{} L{}", info.name, info.level);
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

/// What a picker-owned row does with every optional ProfileSelect field family.
///
/// The `Level` caption/value and bottom `PlayTime` are hidden for every picker row. The remaining
/// fields are row-kind-specific: a save-file row can stage its timestamp into top-right `Location`,
/// metadata rows own `ErStats`, and only the drive-cycle row owns populated `DriveCell_0..25` cells.
pub(crate) struct RowSlotInfo {
    /// Replacement text for the `Location` field (when the file was last written), or `None` to hide
    /// the field -- which is what every non-file row gets, and what a file whose timestamp is
    /// unreadable gets rather than a fabricated date.
    pub(crate) location: Option<String>,
    /// Whether this row has real `ErStats` copy. False on the drive row unless a visible status
    /// message temporarily owns it, so stale parent-folder copy cannot survive row-clip reuse.
    pub(crate) er_stats: bool,
    /// Number of populated drive-strip cells on this row. Zero outside the drive row and while a
    /// visible status message temporarily owns its field band.
    pub(crate) drive_cell_count: usize,
    /// Focus target whose geometry the row's native animated Cursor must follow.
    pub(crate) drive_strip_focus: Option<er_save_picker::DriveStripFocus>,
}

/// What the browse picker wants done with ProfileSelect row `row`'s per-slot info fields.
///
/// `None` when the picker does NOT own the rows. That is the load-bearing half of the scope: the
/// vanilla character-slot views, the title-screen Load Game list first among them, render from the
/// game's own records and must be left exactly as the game draws them. Same ownership gate as
/// [`save_picker_browse_stats_lines`], so the two cannot disagree about who owns a row.
pub(crate) fn save_picker_row_slot_info(row: usize) -> Option<RowSlotInfo> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let (last_saved, er_stats, drive_cell_count, drive_strip_focus) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let model = guard.as_ref()?;
        let has_auxiliary_lines = model.row_auxiliary_lines(row).is_some();
        let drive_cell_count = if model.drive_row() == Some(row) && !has_auxiliary_lines {
            model.drive_strip_cell_count()
        } else {
            0
        };
        let drive_strip_focus = (drive_cell_count > 0)
            .then(|| model.drive_strip_focus())
            .flatten();
        (
            model.row_last_saved(row),
            has_auxiliary_lines || model.row_file_characters(row).is_some(),
            drive_cell_count,
            drive_strip_focus,
        )
    };
    Some(RowSlotInfo {
        location: last_saved.and_then(save_picker_last_saved_text),
        er_stats,
        drive_cell_count,
        drive_strip_focus,
    })
}

/// Render one file's modification time as the row's last-saved text, in local time.
/// `None` when the stamp predates the epoch or the OS cannot give a local offset for it -- the row
/// then hides the field rather than showing a date we cannot stand behind.
pub(crate) fn save_picker_last_saved_text(modified: std::time::SystemTime) -> Option<String> {
    let secs = modified
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let secs = i64::try_from(secs).ok()?;
    crate::experiments::save_picker::format_last_saved(secs, unsafe {
        local_utc_offset_seconds(secs)
    }?)
}

/// The local zone's offset from UTC at the instant `utc_secs`, in seconds.
///
/// Asks WINDOWS rather than assuming, and asks about THAT INSTANT rather than about now:
/// `SystemTimeToTzSpecificLocalTime` applies the zone's DST rules for the given date, so a save
/// written on the other side of a DST boundary still renders the wall-clock time it was written at.
/// (Comparing `GetLocalTime` to `GetSystemTime` would give only the CURRENT offset and misdate every
/// file from the other side of the boundary by an hour.) The offset comes back as a number, which is
/// all the pure formatter needs -- that is what keeps the rendering unit-testable.
pub(crate) unsafe fn local_utc_offset_seconds(utc_secs: i64) -> Option<i64> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };

    /// 100ns ticks per second, and the seconds between the FILETIME (1601) and Unix (1970) epochs.
    const TICKS_PER_SECOND: i64 = 10_000_000;
    const FILETIME_EPOCH_TO_UNIX_SECONDS: i64 = 11_644_473_600;

    fn to_filetime(secs: i64) -> Option<FILETIME> {
        let ticks = secs
            .checked_add(FILETIME_EPOCH_TO_UNIX_SECONDS)?
            .checked_mul(TICKS_PER_SECOND)
            .and_then(|t| u64::try_from(t).ok())?;
        Some(FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        })
    }

    let utc_ft = to_filetime(utc_secs)?;
    let mut utc_st = SYSTEMTIME::default();
    unsafe { FileTimeToSystemTime(&utc_ft, &mut utc_st) }.ok()?;
    let mut local_st = SYSTEMTIME::default();
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc_st, &mut local_st) }.ok()?;
    // Reading the local wall clock back as if it were UTC turns it into "unix seconds shifted by the
    // offset", so the difference IS the offset the zone applied at that instant.
    let mut local_ft = FILETIME::default();
    unsafe { SystemTimeToFileTime(&local_st, &mut local_ft) }.ok()?;
    let local_ticks =
        (u64::from(local_ft.dwHighDateTime) << 32) | u64::from(local_ft.dwLowDateTime);
    let local_secs =
        i64::try_from(local_ticks / TICKS_PER_SECOND as u64).ok()? - FILETIME_EPOCH_TO_UNIX_SECONDS;
    Some(local_secs - utc_secs)
}

#[cfg(test)]
mod save_picker_row_slot_info_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// SCOPE PROOF for the row Level/PlayTime rework: with no picker owning the rows -- the state
    /// the vanilla character-slot views run in, the title-screen Load Game list among them -- the
    /// gate answers `None` for every row, and `None` is the only answer the populate hook treats as
    /// "leave this row exactly as the game drew it". A regression that made the suppression or the
    /// last-saved text global would have to make this return `Some` here first.
    #[test]
    fn no_picker_means_no_row_is_ever_classified() {
        assert_eq!(
            SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst),
            0,
            "no picker session may be active in a unit test"
        );
        for row in 0..crate::experiments::save_picker::PICKER_ROW_COUNT {
            assert!(
                save_picker_row_slot_info(row).is_none(),
                "row {row} was classified without a picker owning the rows"
            );
            assert!(
                save_picker_browse_stats_lines(row).is_none(),
                "row {row} got browse stats without a picker owning the rows"
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
                    crate::mh::leak_installed_hook(hook);
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
    SAVE_PICKER_REBUILD_PENDING_DIALOG.store(0, Ordering::SeqCst);
    SAVE_PICKER_SCROLLBAR_LAST_SYNC.store(usize::MAX, Ordering::SeqCst);
    SAVE_PICKER_SCROLLBAR_DEAD_PROXY_SKIPS.store(0, Ordering::SeqCst);
    save_picker_reset_path_editor_state();
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
    if let Some(preferred) = crate::config::preferred_save_picker_dir_now()
        && let Some(text) = preferred.to_str()
    {
        let windows = PathBuf::from(save_picker_windows_path_string(text));
        if windows.is_dir() {
            return windows;
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
