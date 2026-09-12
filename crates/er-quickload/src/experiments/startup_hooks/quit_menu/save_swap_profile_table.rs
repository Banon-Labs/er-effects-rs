use super::*;

pub(crate) unsafe fn system_quit_apply_foreign_profile_summary_preview(
    base: usize,
    bytes: &[u8],
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == null {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: cannot preview replacement save -- live ProfileSummary unavailable"
        ));
        return 0;
    }
    // The snapshot is taken fresh on every preview, never reused.
    //
    // This used to short-circuit on `!summary_snapshot.is_empty() && summary_ptr == summary`, which
    // made the snapshot a process-lifetime latch: the ProfileSummary allocation is reused across
    // save containers, so a snapshot taken minutes and one container earlier was kept and would
    // have been written back as "the user's real rows" -- the previous character's stats, which is
    // the remaining half of `er-effects-rs-fmy6`. Re-reading costs one memcpy per pick.
    //
    // Where the pre-call records live is the subtlety. The save picker may have its browse-row
    // labels in the live allocation right now (a pick arrives while the picker still owns the
    // window), and `write_profile_summary_records_from_save_bytes` uses this image as the
    // structural template for slots it cannot source -- so reading the live allocation blind would
    // seed the preview from `[ new ]`. The picker's own snapshot is the game's records, so prefer
    // it whenever one is live for this same allocation.
    let summary_snapshot = {
        let mut st = system_quit_save_swap_lock();
        let from_rows = st.rows_staged
            && st.rows_summary_ptr == summary
            && st.rows_snapshot.len() == PROFILE_SUMMARY_TOTAL_BYTES;
        let snapshot = if from_rows {
            st.rows_snapshot.clone()
        } else {
            unsafe {
                core::slice::from_raw_parts(summary as *const u8, PROFILE_SUMMARY_TOTAL_BYTES)
                    .to_vec()
            }
        };
        st.summary_ptr = summary;
        st.summary_snapshot = snapshot.clone();
        snapshot
    };

    let (mask, preview_stats) = unsafe {
        write_profile_summary_records_from_save_bytes(base, summary, &summary_snapshot, bytes)
    };
    if mask != 0 {
        {
            let mut st = system_quit_save_swap_lock();
            st.candidate_slot_mask = mask;
            st.candidate_stats_utf16 = preview_stats;
            st.preview_applied = true;
            // Ownership HANDOFF, and only now that the preview actually took. The browse rows are
            // gone from the allocation (the writer above zeroes all ten records first) and this
            // preview's snapshot carries the same game records the staging snapshot did, so the
            // preview owns the backout from here. Transferring only on success matters: a preview
            // that found no readable slot leaves the picker on screen, and the rows restore below
            // is what puts its listing back.
            st.rows_staged = false;
            st.rows_summary_ptr = 0;
            st.rows_snapshot = Vec::new();
        }
        // The rows about to be drawn describe **this** save, so our CACHES must too. The native
        // ProfileSummary above now holds the previewed save's records, but the name and the whole
        // attribute line on each row come from `PROFILE_SLOT_*_CACHE`, which was a process-lifetime
        // latch: without this the picker showed the new save's levels and locations under the old
        // save's names and stats. `bytes` is the previewed save itself, so this is a parse, not a
        // second ~26 MB read.
        let decoded =
            crate::experiments::startup_hooks::loading_cover::load_profile_slot_caches_from_bytes(
                bytes,
                "picker-previewed save",
            );
        let reloads = PROFILE_SLOT_CACHE_PREVIEW_RELOADS.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-save-swap: per-slot stats/name caches reloaded from the previewed save ({decoded}/10 slots, reloads={reloads})"
        ));
        PROFILE_STATS_PREVIEW_ROW_CURSOR.store(0, Ordering::SeqCst);
        // Park the cursor on a row this save actually has. The rows about to be built describe
        // only the slots in `mask` (the native builder pushes a row per set
        // `saveSlotsStates[slot]`), and the dialog's constructor leaves the cursor on row 0 --
        // which, for a save whose lowest character is not slot 0, is either another character's
        // row or the live session's own. Requested here, applied by the per-frame
        // `05_010_ProfileSelect` run through the game's own `SelectSaveSlot`, because the dialog
        // this preview is for is generally not built yet.
        if let Some(target) = er_quit_menu_core::profile_rows::preview_cursor_slot(mask as u32) {
            SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_SLOT.store(target, Ordering::SeqCst);
        }
        let refresh: unsafe extern "system" fn() = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    PROFILE_RENDERER_REFRESH_RVA,
                    "PROFILE_RENDERER_REFRESH_RVA",
                ) {
                    Some(address) => address,
                    None => return 0,
                },
            )
        };
        unsafe { refresh() };
    } else {
        // Nothing PREVIEWED, but the records are already destroyed.
        // `write_profile_summary_records_from_save_bytes` zeroes all ten records and their
        // occupancy bytes before it discovers whether the container has a readable slot, so a
        // refused pick leaves the live summary blank. The caller keeps the picker open so the user
        // can choose another file -- which needs its rows back. (The old comment at that call site
        // claimed "our browse rows were untouched"; they never were.)
        unsafe { save_picker_restore_staged_row_records("preview-found-no-slots") };
    }
    mask
}

pub(crate) fn system_quit_save_swap_restore_original_file(
    st: &SystemQuitSaveSwapState,
    reason: &str,
) -> bool {
    if st.path.is_empty() || st.original_bytes.is_empty() {
        return false;
    }
    match fs::write(&st.path, &st.original_bytes) {
        Ok(()) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: restored active save file for {reason} path='{}' len={} hash=0x{:016x}",
                st.path,
                st.original_bytes.len(),
                st.original_hash
            ));
            true
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: FAILED to restore active save file for {reason} path='{}': {err}",
                st.path
            ));
            false
        }
    }
}

/// Is a foreign save's summary currently on screen (previewed, not yet committed)?
///
/// The row presentation needs this to answer one question correctly: whose name belongs on slot 0.
/// The transient current-player row is built with slot index 0 (`FUN_1408753f0` ->
/// `FUN_1408759e0(summary, 0, &name, pgd->level)`), so slot 0 normally prefers the live character's
/// name. While a foreign save is previewed, slot 0 is that save's slot 0 instead, and preferring the
/// live name puts the loaded character's name on another save's character -- observed 2026-08-07 as
/// "Maddened Bean, RL 100" where RL 100, the attributes and the location were all angrE's.
pub(crate) fn system_quit_foreign_preview_active() -> bool {
    let st = system_quit_save_swap_lock();
    // The question this answers is "do the live ProfileSummary records describe something other
    // than the loaded character", and the save picker's staged browse rows are as much "something
    // other" as a foreign save's records. It used to read `preview_applied` alone, which was true
    // during row staging only because staging set that flag -- an accident of the conflation the
    // `rows_staged` split removed. Naming the staging explicitly keeps the row presentation
    // behaving exactly as it did while the restore concerns stay separate.
    st.rows_staged || (st.preview_applied && !st.committed)
}

/// Put the game's own `CS::ProfileSummary` records back after the in-game save picker wrote its
/// browse-row labels over them. Returns true when a restore was actually performed.
///
/// Unconditional by design -- It is not gated on `committed`, and must not be.
///
/// The defect this exists to close (live run 2026-08-29): a save-destination picker staged four row
/// records at +313751ms; `committed` had been set at +118359ms by an unrelated cross-file load, and
/// the only thing that ever clears it sits past the guard that reads it, so it is sticky for the
/// life of the process. Every restore after that point silently no-op'd -- zero
/// `restored live ProfileSummary snapshot` lines in a 600 MB log -- and the picker's labels stayed
/// in the records. The user's next three loading screens rendered `[..] EldenRing` and `[ new ]`
/// as character names beside `RL 0`, and the loading portrait drew nothing because the record it
/// had to build from was a zeroed row.
///
/// A committed foreign preview genuinely must survive (its records are what the game is about to
/// load). Browse-row labels never must: they are UI, they describe no character, and there is no
/// state in which leaving them in a game-owned structure is correct.
///
/// # Safety
///
/// Writes `PROFILE_SUMMARY_TOTAL_BYTES` through a raw pointer, so it writes only when the live
/// summary pointer still equals the allocation the snapshot came from -- a stricter check than the
/// preview restore's bare `>= 0x10000`, because a container reallocation between staging and
/// restore would otherwise be a use-after-free. Menu/game-thread only, like every other writer of
/// these records.
pub(crate) unsafe fn save_picker_restore_staged_row_records(reason: &str) -> bool {
    let mut st = system_quit_save_swap_lock();
    if !st.rows_staged {
        return false;
    }
    let live = unsafe { system_quit_profile_summary_ptr() };
    let target = st.rows_summary_ptr;
    let snapshot_len = st.rows_snapshot.len();
    // "CANNOT READ THE SUMMARY RIGHT NOW" is not "THE SUMMARY IS GONE". `GameDataMan+0x78` reads as
    // 0 through the whole clean-title window, and consuming the latch on that reading would throw
    // away the only copy of the user's records over a pointer that is about to come back. Keep it
    // armed and let the per-frame sweep retry; the log is rate-limited because that sweep runs every
    // frame.
    if live < 0x10000 {
        let n = SAVE_PICKER_ROW_RECORDS_RESTORE_DEFERRED.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 2 || n.is_power_of_two() {
            append_autoload_debug(format_args!(
                "save-picker: DEFERRED the staged-row restore for {reason} -- the live ProfileSummary is unreadable (0x{live:x}) #{n}; the snapshot stays armed and the per-frame sweep retries"
            ));
        }
        return false;
    }
    let writable =
        target >= 0x10000 && target == live && snapshot_len == PROFILE_SUMMARY_TOTAL_BYTES;
    if writable {
        unsafe {
            core::ptr::copy_nonoverlapping(
                st.rows_snapshot.as_ptr(),
                target as *mut u8,
                snapshot_len,
            );
        }
    }
    // Consume the latch now, written or not. `live` is readable and is not the allocation we
    // snapshotted, so that allocation is gone: there is nothing left to put back, and keeping its
    // image armed would only make a later restore write a dead container's records into whatever
    // now occupies the address.
    st.rows_staged = false;
    st.rows_summary_ptr = 0;
    st.rows_snapshot = Vec::new();
    drop(st);
    // The staging zeroed these per slot; the restored records are the game's own, whose faces this
    // preview-only fingerprint array says nothing about.
    for face_hash in PROFILE_PREVIEW_FACE_HASH
        .iter()
        .take(TITLE_PROFILE_SLOT_COUNT)
    {
        face_hash.store(0, Ordering::SeqCst);
    }
    if writable
        && game_module_base().is_ok()
        && let Some(address) = crate::experiments::gated_game_fn(
            PROFILE_RENDERER_REFRESH_RVA,
            "PROFILE_RENDERER_REFRESH_RVA",
        )
    {
        let refresh: unsafe extern "system" fn() = unsafe { std::mem::transmute(address) };
        unsafe { refresh() };
    }
    SAVE_PICKER_ROW_RECORDS_RESTORED.fetch_add(1, Ordering::SeqCst);
    if !writable {
        SAVE_PICKER_ROW_RECORDS_RESTORE_UNWRITABLE.fetch_add(1, Ordering::SeqCst);
    }
    SAVE_PICKER_STAGED_ROW_COUNT.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker: restored the game's live ProfileSummary records over the staged browse rows for {reason} summary=0x{target:x} live=0x{live:x} bytes={snapshot_len} written={writable}"
    ));
    true
}

pub(crate) unsafe fn system_quit_save_swap_restore_profile_summary(reason: &str) {
    // Two independent restores, in order. The picker's browse rows come back unconditionally; the
    // foreign-save preview's backout keeps its `committed` suppression, because a committed preview
    // is the save the game is about to load. Splitting them is the fix for the sticky-`committed`
    // defect described on `save_picker_restore_staged_row_records`. They are mutually exclusive in
    // practice (a successful preview takes ownership of the summary from the staging), and where
    // both were somehow live the two snapshots hold the same game records, so the order is safe
    // either way.
    unsafe { save_picker_restore_staged_row_records(reason) };
    let mut st = system_quit_save_swap_lock();
    if !st.preview_applied || st.committed {
        return;
    }
    if st.summary_ptr >= 0x10000 && !st.summary_snapshot.is_empty() {
        unsafe {
            core::ptr::copy_nonoverlapping(
                st.summary_snapshot.as_ptr(),
                st.summary_ptr as *mut u8,
                st.summary_snapshot.len(),
            );
        }
        if let Ok(_base) = game_module_base() {
            let refresh: unsafe extern "system" fn() = unsafe {
                std::mem::transmute(
                    match crate::experiments::gated_game_fn(
                        PROFILE_RENDERER_REFRESH_RVA,
                        "PROFILE_RENDERER_REFRESH_RVA",
                    ) {
                        Some(address) => address,
                        None => return,
                    },
                )
            };
            unsafe { refresh() };
        }
        append_autoload_debug(format_args!(
            "system-quit-save-swap: restored live ProfileSummary snapshot for {reason} summary=0x{:x} bytes={}",
            st.summary_ptr,
            st.summary_snapshot.len()
        ));
    }
    // Symmetric with the reload on preview: the summary is the original save's again, so the caches
    // describing the previewed save must go. Dropped rather than reloaded because the bytes of the
    // active save are not in hand here -- the next row populate reads them.
    crate::experiments::startup_hooks::loading_cover::invalidate_profile_slot_caches(reason);
    // ...and so must a pending cursor move: it named a slot of the save that is no longer previewed,
    // so applying it after the restore would drive the cursor onto an unrelated character's row.
    SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_SLOT.store(
        SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_NONE,
        Ordering::SeqCst,
    );
    // The restored snapshot's records are the original save's characters -- the foreign preview face
    // fingerprints no longer describe any slot, and neither does the preview's record of which slots
    // it could not source a place name for.
    for face_hash in PROFILE_PREVIEW_FACE_HASH
        .iter()
        .take(TITLE_PROFILE_SLOT_COUNT)
    {
        face_hash.store(0, Ordering::SeqCst);
    }
    PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.store(0, Ordering::SeqCst);
    let _ = system_quit_save_swap_restore_original_file(&st, reason);
    *st = SystemQuitSaveSwapState::default();
}

pub(crate) unsafe fn system_quit_save_swap_poll_preview(base: usize) {
    let tick = SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.fetch_add(1, Ordering::SeqCst);
    if !tick.is_multiple_of(SYSTEM_QUIT_SAVE_SWAP_POLL_INTERVAL_TICKS) {
        return;
    }
    let (path, original_hash, original_len, original_modified_ns, preview_applied) = {
        let st = system_quit_save_swap_lock();
        if !st.armed || st.committed || st.path.is_empty() {
            return;
        }
        (
            st.path.clone(),
            st.original_hash,
            st.original_len,
            st.original_modified_ns,
            st.preview_applied,
        )
    };
    if preview_applied {
        return;
    }
    let Some((len, modified_ns)) = system_quit_file_stamp(&path) else {
        return;
    };
    if len == original_len && modified_ns == original_modified_ns {
        return;
    }
    let Ok(mut bytes) = fs::read(&path) else {
        return;
    };
    let raw_hash = system_quit_hash_bytes(&bytes);
    if raw_hash == original_hash {
        return;
    }
    // Validate before restoring the active redirected save. A partial copy must not be captured as a
    // foreign preview, and the old in-world save must remain the write target until the user commits.
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: replacement candidate changed but is not a valid BND4 yet path='{path}' len={len} hash=0x{raw_hash:016x}; waiting"
        ));
        return;
    }
    normalize_save_bytes_to_active_steam_id(base, &mut bytes, "system-quit-polled-candidate");
    let hash = system_quit_hash_bytes(&bytes);
    {
        let st = system_quit_save_swap_lock();
        if !system_quit_save_swap_restore_original_file(&st, "candidate-captured") {
            return;
        }
    }
    let mask = unsafe { system_quit_apply_foreign_profile_summary_preview(base, &bytes) };
    if mask == 0 {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: valid replacement candidate had no readable character slots path='{path}' len={len} hash=0x{hash:016x}; active file restored, preview not applied"
        ));
        return;
    }
    let mut st = system_quit_save_swap_lock();
    st.candidate_bytes = bytes;
    st.candidate_hash = hash;
    st.candidate_slot_mask = mask;
    st.preview_applied = true;
    append_autoload_debug(format_args!(
        "system-quit-save-swap: applied FOREIGN ProfileSummary preview from replacement path='{path}' len={len} hash=0x{hash:016x} slot_mask=0x{mask:x}; active save file restored until the user selects a foreign slot"
    ));
}

/// Park the live `05_010_ProfileSelect` cursor on the slot a foreign preview asked for, through the
/// game's own `CS::ProfileLoadDialog::SelectSaveSlot`.
///
/// A no-op unless a preview armed a target. It is retried from the per-frame ProfileSelect run and
/// cleared the moment the native call reports it found a row, so it moves the cursor exactly once
/// per preview and never fights a user who then navigates.
pub(crate) unsafe fn system_quit_park_profile_select_cursor(base: usize, dialog: usize) {
    let target = SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_SLOT.load(Ordering::SeqCst);
    if target == SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_NONE {
        return;
    }
    // The preview lands while the file browser still owns this same 05_010 window, and its rows are
    // directory entries, not character slots. Wait for the browser to hand the window back before
    // touching a cursor that currently means "which file".
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0 {
        return;
    }
    if !unsafe { er_title_flow::profile_dialog_select_save_slot(base, dialog, target) } {
        // The rows are not built yet, or this save has no row for that slot. Either way leave the
        // request armed: the next frame retries, and a restore clears it.
        return;
    }
    SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_SLOT.store(
        SYSTEM_QUIT_PROFILE_SELECT_CURSOR_TARGET_NONE,
        Ordering::SeqCst,
    );
    let cursor = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1);
    let bound = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
    append_autoload_debug(format_args!(
        "system-quit-save-swap: parked ProfileSelect cursor on the previewed save's slot {target} via native SelectSaveSlot dialog=0x{dialog:x} cursor={cursor} bound={bound}"
    ));
}

pub(crate) unsafe fn system_quit_save_swap_prepare_selected_slot(slot: i32) -> Result<bool, ()> {
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: prepare selected slot skipped -- out-of-range slot={slot}"
        ));
        return Ok(false);
    }
    let mut st = system_quit_save_swap_lock();
    if !st.preview_applied || st.committed {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: prepare selected slot skipped slot={slot} preview_applied={} committed={} armed={} path_set={} candidate_len={} mask=0x{:x}",
            st.preview_applied,
            st.committed,
            st.armed,
            !st.path.is_empty(),
            st.candidate_bytes.len(),
            st.candidate_slot_mask
        ));
        return Ok(false);
    }
    let bit = 1usize << slot as usize;
    if st.candidate_slot_mask & bit == 0 {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: refusing ProfileSelect activation for slot {slot}; foreign preview active but slot bit is absent mask=0x{:x}",
            st.candidate_slot_mask
        ));
        return Err(());
    }
    if st.path.is_empty() || st.candidate_bytes.is_empty() {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: refusing ProfileSelect activation for slot {slot}; foreign preview state incomplete path_set={} candidate_len={} mask=0x{:x}",
            !st.path.is_empty(),
            st.candidate_bytes.len(),
            st.candidate_slot_mask
        ));
        return Err(());
    }
    match write_save_bytes_for_overwrite(&st.path, &st.candidate_bytes) {
        Ok(()) => {
            st.committed = true;
            st.recommitted = false;
            st.armed = false;
            append_autoload_debug(format_args!(
                "system-quit-save-swap: committed foreign save before slot activation path='{}' slot={slot} len={} hash=0x{:016x}; fresh deserialize will read this file",
                st.path,
                st.candidate_bytes.len(),
                st.candidate_hash
            ));
            Ok(true)
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: FAILED to commit foreign save for slot {slot} path='{}': {err}; blocking activation to avoid loading stale/original bytes",
                st.path
            ));
            Err(())
        }
    }
}

pub(crate) fn make_save_file_writable_for_overwrite(path: &str) {
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            // Windows-only DLL: this clears FILE_ATTRIBUTE_READONLY, the one bit we mean. The
            // lint is about the UNIX behaviour (0o666 for everyone), which cannot occur here.
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

pub(crate) fn write_save_bytes_for_overwrite(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    make_save_file_writable_for_overwrite(path);
    fs::write(path, bytes)
}

/// Re-commit the foreign candidate bytes after the game's return-title save completes (bc4 terminal).
/// The activation-time commit is CLOBBERED by that save whenever the picked slot shares the active
/// character's slot index: the return-title chain sets saveRequested and the game re-writes the active
/// slot (+ profile summary) into the active file ~400ms after our write (gm-snap: bc4 1 -> save_state=1
/// -> bc4 terminal), so a same-slot switch fresh-deserialized the original character (user-reported
/// 2026-07-06, run seamless-save-smoke-20260706-144801: two same-slot-0 picks both reloaded the
/// resident character; Face-identity mismatch #1 confirmed it at RAM level before the pixels did).
/// Different-slot switches always survived because the clobber only rewrites the active slot's
/// USER_DATA entry. By bc4-terminal the save write has finished, and the fresh deserialize is still
/// seconds away at the clean title, so a second write of the pristine candidate bytes wins. Nothing
/// meaningful is lost: System-Quit already saved the old character into their own file before
/// ProfileSelect opened; the return-title re-save was landing in the wrong (foreign) file anyway.
/// Idempotent per switch via the `recommitted` latch (the terminal block can re-enter when the final
/// functor submit defers).
pub(crate) fn system_quit_save_swap_recommit_after_return_title_save() {
    let candidate = {
        let mut st = system_quit_save_swap_lock();
        if !st.committed || st.recommitted || st.path.is_empty() || st.candidate_bytes.is_empty() {
            return;
        }
        match write_save_bytes_for_overwrite(&st.path, &st.candidate_bytes) {
            Ok(()) => {
                st.recommitted = true;
                append_autoload_debug(format_args!(
                    "system-quit-save-swap: RE-committed foreign save after return-title save (bc4 terminal) path='{}' len={} hash=0x{:016x}; the game's return-title save had re-written the ACTIVE slot over the activation-time commit",
                    st.path,
                    st.candidate_bytes.len(),
                    st.candidate_hash
                ));
            }
            Err(err) => {
                append_autoload_debug(format_args!(
                    "system-quit-save-swap: FAILED to re-commit foreign save after return-title save path='{}': {err}; a same-slot switch will fresh-deserialize the clobbered ACTIVE slot",
                    st.path
                ));
                return;
            }
        }
        // Cloned rather than `mem::take`n; see the callee's doc for why emptying it would race.
        st.candidate_bytes.clone()
    };
    let Ok(base) = game_module_base() else {
        return;
    };
    let summary = unsafe { system_quit_profile_summary_ptr() };
    unsafe { reapply_profile_summary_after_return_title_save(base, summary, &candidate) };
}

/// The game-owned save file a Load-Save-Profiles pick has committed foreign character bytes into this
/// switch, or `None` when no runtime foreign pick is active (normal boot / config-only autoload).
///
/// When the human-driven "Load Save Profiles" path activates a foreign slot,
/// `system_quit_save_swap_prepare_selected_slot` overwrites the active `%APPDATA%/EldenRing/<steamid>/
/// ER0000.{sl2,co2}` file (`st.path` -- the game-owned default, never the read-only picked source or
/// the configured `save_file`) with the picked slot's candidate bytes and sets `committed = true`. The
/// own-load feed uses this to read the committed file instead of the configured `save_file` for that
/// pick's load (drive.rs `own_load_read_sl2_bytes`): a runtime pick overrides the config default for
/// exactly one load. Returns `None` unless the commit actually landed and the path/candidate are still
/// present, so a normal boot autoload (no pick) still reads the configured `save_file` unchanged.
pub(crate) fn system_quit_committed_foreign_save_path() -> Option<String> {
    let st = system_quit_save_swap_lock();
    if st.committed && !st.path.is_empty() && !st.candidate_bytes.is_empty() {
        Some(st.path.clone())
    } else {
        None
    }
}
