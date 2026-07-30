// The OS common file dialog, as an opt-in alternative to the in-game 05_010 browser
// (`os_native_save_picker = true`). Recovered from `ca846fa1`, which REMOVED this shape citing the
// context switch out of the game -- not a hang -- so the plain blocking call is the only variant
// with field evidence behind it.
//
// This module converts strings and calls comdlg32. It reads no game pointers, calls no game
// function, and dereferences nothing from `game_module_base()`. Everything that touches the game
// happens on the CALLER's side of the return, in code that already runs in the right ownership
// context. That is rule H3 and it is what makes the rest of the hazard analysis tractable.
//
// THREADING. The dialog is called inline and synchronously on the thread that already owns "open a
// picker for this intent": the row-action hook (menu thread) for a load, the menu pump for a
// destination. No worker thread, no `CoInitialize`, no cross-thread hand-off. Three reasons:
// the removed shape did exactly this and functioned; every consumer of the returned pick is
// menu-thread-only by documentation (`system_quit_ingest_picked_save` writes ProfileSummary records
// and refreshes the renderer, `save_flow_submit_box` says "MENU-THREAD ONLY"); and a cross-thread
// `hwndOwner` would be WORSE for input, because it disables the game window from another thread
// while the game keeps polling raw input, so the System>Quit menu underneath would receive the
// keystrokes typed into the dialog. Blocking the menu pump means the menus cannot process anything
// at all, which is the modality we want.
//
// The game task keeps ticking meanwhile -- see `save_flow_next_stage_ticks`, which is why the
// flow's deadlines are frozen while `SAVE_PICKER_OS_DIALOG_OPEN` is set.

use windows::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_DONTADDTORECENT, OFN_EXPLORER,
    OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_NOTESTFILECREATE, OFN_PATHMUSTEXIST,
    OPEN_FILENAME_FLAGS, OPENFILENAMEW,
};

pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_CANCEL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_CLOSED_WITH_PATH;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_ERROR_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_LAST_ERROR;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_LAST_REJECT_REASON;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_OWNER_HWND;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_REJECT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_REOPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_REOPEN_EXHAUSTED;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_SAVELIKE_OPENS;

/// Path buffer handed to comdlg32. `MAX_PATH` is not the limit for an explorer-style dialog; the
/// recovered code used 1024 wide units and a Wine `Z:\...` spelling of a deep Linux path can be
/// long, so keep the same generous buffer.
const OS_PICK_PATH_UNITS: usize = 1024;

/// Consecutive INVALID picks tolerated before the loop gives up and takes the cancel path.
///
/// This bound is not about user patience -- eight is generous for a human. It exists because
/// comdlg32 might fail INSTANTLY: Wine's is a reimplementation, and a dialog that returns at once
/// with a stale path would spin this loop at full speed on the thread that owns the menu pump, an
/// unbreakable hang. Only invalid PICKS reopen; a cancel or a comdlg32 failure never does.
const SAVE_PICKER_OS_MAX_REOPENS: usize = 8;

/// What one dialog invocation produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OsPickOutcome {
    /// A path, in the Windows form comdlg32 returned it.
    Picked(String),
    /// The user dismissed the dialog (`FALSE`, extended error 0).
    Cancelled,
    /// comdlg32 itself failed (`FALSE`, non-zero extended error), or returned TRUE with no path.
    Failed { error: u32 },
}

/// Read comdlg32's `FALSE` return correctly. A `FALSE` means EITHER the user cancelled (extended
/// error 0) OR the dialog failed (non-zero) -- collapsing the two would make a broken comdlg32 look
/// like a user decision, and only a failure is a bug of ours. Neither reopens.
///
/// Pure so the gate can test it: the real call feeds it `returned`, `CommDlgExtendedError()` and the
/// buffer's decoded path.
fn classify_os_outcome(returned: bool, extended_error: u32, path: Option<String>) -> OsPickOutcome {
    match (returned, path) {
        (true, Some(path)) if !path.is_empty() => OsPickOutcome::Picked(path),
        // TRUE with nothing in the buffer is not a user decision; it is a dialog that lied.
        (true, _) => OsPickOutcome::Failed { error: 0 },
        (false, _) if extended_error == 0 => OsPickOutcome::Cancelled,
        (false, _) => OsPickOutcome::Failed {
            error: extended_error,
        },
    }
}

/// Whether an outcome should reopen the dialog. ONLY an invalid pick does, and only under the bound.
fn should_reopen(outcome: &OsPickOutcome, pick_was_valid: bool, attempts: usize) -> bool {
    matches!(outcome, OsPickOutcome::Picked(_))
        && !pick_was_valid
        && attempts < SAVE_PICKER_OS_MAX_REOPENS
}

/// Double-NUL-terminated comdlg32 filter for the active flavor's extensions, e.g.
/// `"Elden Ring save (*.co2;*.sl2)\0*.co2;*.sl2\0\0"`.
///
/// DISPLAY-ONLY. The removed code's own comment said so: the dialog returns whatever path the user
/// types, filter or not. `save_picker_accepts` is what actually decides, which is why there is no
/// "All files" escape hatch here -- it would change nothing.
fn os_dialog_filter(extensions: &[&str]) -> Vec<u16> {
    let patterns: Vec<String> = extensions
        .iter()
        .map(|ext| format!("*.{}", ext.trim_start_matches('.')))
        .collect();
    let joined = patterns.join(";");
    let mut out: Vec<u16> = format!("Elden Ring save ({joined})")
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
    out.extend(joined.encode_utf16());
    out.push(0);
    out.push(0);
    out
}

/// Guard for the ONE dialog claim. Its `Drop` clears the latch, so an unwind cannot leave the flow
/// permanently frozen (the latch is the tick-freeze predicate).
struct OsDialogClaim;

impl OsDialogClaim {
    /// Claim the right to open a dialog, or `None` if one is already up.
    ///
    /// COMPARE-EXCHANGE, not a store, and that distinction is the whole point (H1). A modal common
    /// dialog runs its own `GetMessage`/`DispatchMessage` loop for the CALLING thread, and a
    /// dispatched message can re-enter the game's window proc, its menu code, and therefore our own
    /// row-action detour -- which would open a second dialog underneath the first, or start a second
    /// save flow. Only the first caller proceeds; a re-entrant one bails immediately. Same
    /// once-claim pattern the repo already needed in `system_quit_ownership_repro`.
    fn claim() -> Option<Self> {
        if SAVE_PICKER_OS_DIALOG_OPEN
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            append_autoload_debug(format_args!(
                "save-picker-os: refusing a re-entrant dialog open -- one is already up (comdlg32 pumped a message back into our own row action)"
            ));
            return None;
        }
        Some(Self)
    }
}

impl Drop for OsDialogClaim {
    fn drop(&mut self) {
        SAVE_PICKER_OS_DIALOG_OPEN.store(0, Ordering::SeqCst);
    }
}

/// The `hwndOwner` for a dialog: the game's own main window, so a window manager keeps the dialog
/// in front of it. Never `hooks::own_window()` -- see [`game_main_window`].
///
/// A null result is NOT fatal: pass it through and log `owner=none`, so a report can distinguish
/// "we passed no owner" from "we passed one and the dialog still went behind the game".
fn os_dialog_owner() -> HWND {
    let hwnd = game_main_window();
    SAVE_PICKER_OS_OWNER_HWND.store(hwnd.0 as usize, Ordering::SeqCst);
    hwnd
}

/// Windows-form path decoded out of the dialog's buffer, up to its NUL.
fn os_pick_path_from_buffer(buffer: &[u16]) -> Option<String> {
    let end = buffer.iter().position(|unit| *unit == 0)?;
    if end == 0 {
        return None;
    }
    String::from_utf16(&buffer[..end]).ok()
}

/// Run ONE dialog. `save_as` selects `GetSaveFileNameW` over `GetOpenFileNameW` and the Save-As flag
/// set; `leaf` pre-fills the filename field (empty for an Open).
///
/// H2 -- NO LOCK OF OURS MAY BE ALIVE ACROSS THIS CALL. The dialog's own file I/O re-enters our
/// `CreateFileW` detour ON THIS THREAD, and that detour takes `save_dest_redirect_lock()` and logs;
/// `save_dest_redirect_for_open`'s own doc states the rule ("a second lock acquisition would
/// deadlock the save worker"), and commit `a02a274d` is the same class one level down in the logger.
/// This signature is the structural enforcement: every parameter is an OWNED `String`/`&str`, so no
/// `MutexGuard` can be borrowed through it.
fn os_dialog_run(
    save_as: bool,
    start_dir: &str,
    leaf: &str,
    extensions: &[&str],
    commit_window_armed: bool,
) -> OsPickOutcome {
    let filter = os_dialog_filter(extensions);
    let title: Vec<u16> = if save_as {
        "Save Game to...".encode_utf16().chain(core::iter::once(0)).collect()
    } else {
        "Load Save Profiles".encode_utf16().chain(core::iter::once(0)).collect()
    };
    let initial_dir: Vec<u16> = start_dir.encode_utf16().chain(core::iter::once(0)).collect();
    let mut path_buffer = [0u16; OS_PICK_PATH_UNITS];
    for (index, unit) in leaf.encode_utf16().take(OS_PICK_PATH_UNITS - 1).enumerate() {
        path_buffer[index] = unit;
    }
    // OPEN: exactly the flag set `ca846fa1` shipped.
    // SAVE-AS: `OFN_FILEMUSTEXIST` is dropped, because a new destination must be nameable, and
    // `OFN_NOTESTFILECREATE` is added -- without it comdlg32 may create and delete a probe file, and
    // a probe left behind (or a race with our own `target.is_file()`) would make Box3 ask
    // "Overwrite this file?" about a name the user just invented, and could hand a 0-byte file to
    // the seed path. Writability is still caught, without touching the destination, by
    // `save_dest_write_atomic`'s sibling-temp-plus-rename.
    //
    // `OFN_OVERWRITEPROMPT` IS DELIBERATELY ABSENT. Our Box3 is the single overwrite gate; the OS
    // prompt would ask the user the same question twice, and the one that decides is ours. This is
    // the one flag whose ABSENCE is load-bearing.
    let flags: OPEN_FILENAME_FLAGS = if save_as {
        OFN_EXPLORER
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY
            | OFN_NOCHANGEDIR
            | OFN_DONTADDTORECENT
            | OFN_NOTESTFILECREATE
    } else {
        OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY
            | OFN_NOCHANGEDIR
            | OFN_DONTADDTORECENT
    };
    let owner = os_dialog_owner();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR::from_raw(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR::from_raw(path_buffer.as_mut_ptr()),
        nMaxFile: path_buffer.len() as u32,
        lpstrInitialDir: PCWSTR::from_raw(initial_dir.as_ptr()),
        lpstrTitle: PCWSTR::from_raw(title.as_ptr()),
        Flags: flags,
        ..Default::default()
    };
    let frozen_before = SAVE_PICKER_OS_TICKS_FROZEN.load(Ordering::SeqCst);
    let savelike_before = SAVE_PICKER_OS_SAVELIKE_OPENS.load(Ordering::SeqCst);
    let started = Instant::now();
    SAVE_PICKER_OS_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    // The OPENED line is unconditional and pairs with CLOSED below. OPENED with no CLOSED and a
    // responsive game means an invisible dialog; OPENED with no CLOSED and a frozen game means a
    // hung one; a CLOSED pair with result=cancelled means it worked and the user declined.
    append_autoload_debug(format_args!(
        "save-picker-os: dialog OPENED surface={} owner=0x{:x}{} dir='{}' leaf='{}' filter='{}' flags=0x{:x} overwrite_prompt=NOT_SET commit_window_armed={commit_window_armed}",
        if save_as { "save-as" } else { "load" },
        owner.0 as usize,
        if owner.0.is_null() { " (owner=none)" } else { "" },
        system_quit_windows_path_for_log(start_dir),
        leaf,
        extensions.join("/"),
        flags.0
    ));
    let returned = if save_as {
        unsafe { GetSaveFileNameW(&mut ofn) }.as_bool()
    } else {
        unsafe { GetOpenFileNameW(&mut ofn) }.as_bool()
    };
    let extended_error = if returned {
        0
    } else {
        unsafe { CommDlgExtendedError() }.0
    };
    let outcome = classify_os_outcome(
        returned,
        extended_error,
        os_pick_path_from_buffer(&path_buffer),
    );
    match &outcome {
        OsPickOutcome::Cancelled => {
            SAVE_PICKER_OS_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        OsPickOutcome::Failed { error } => {
            SAVE_PICKER_OS_ERROR_COUNT.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_OS_LAST_ERROR.store(*error as usize, Ordering::SeqCst);
        }
        OsPickOutcome::Picked(_) => {
            SAVE_PICKER_OS_CLOSED_WITH_PATH.fetch_add(1, Ordering::SeqCst);
        }
    }
    append_autoload_debug(format_args!(
        "save-picker-os: dialog CLOSED result={} path='{}' after={}ms frozen_ticks={} savelike_opens={}",
        match &outcome {
            OsPickOutcome::Picked(_) => "picked".to_owned(),
            OsPickOutcome::Cancelled => "cancelled".to_owned(),
            OsPickOutcome::Failed { error } => format!("failed(err=0x{error:x})"),
        },
        match &outcome {
            OsPickOutcome::Picked(path) => system_quit_windows_path_for_log(path),
            _ => "<none>".to_owned(),
        },
        started.elapsed().as_millis(),
        SAVE_PICKER_OS_TICKS_FROZEN
            .load(Ordering::SeqCst)
            .saturating_sub(frozen_before),
        SAVE_PICKER_OS_SAVELIKE_OPENS
            .load(Ordering::SeqCst)
            .saturating_sub(savelike_before)
    ));
    outcome
}

/// What one [`os_pick_validated`] call did.
///
/// `Dismissed` and `NotOpened` were a single `None` before, and collapsing them is what let a
/// user's Cancel be retried as though the dialog had never opened (bd `er-effects-rs-rsxi`). They
/// are opposite facts: a dismissal means a dialog RAN and the user answered it, so the request that
/// asked for it is finished; a `NotOpened` means no dialog ran at all, so the request still stands.
enum OsPickResult<T> {
    /// The pick cleared the listing predicate and `stage` ran on it.
    Staged(T),
    /// A dialog ran and came back with no usable pick: the user cancelled, comdlg32 failed, or the
    /// invalid-pick reopen bound gave up. TERMINAL.
    Dismissed,
    /// No dialog ran: the core `CreateFileW` detour is not live yet, or a re-entrant open was
    /// refused because one is already up.
    NotOpened,
}

/// Open the dialog, validate what comes back with the picker's OWN predicate, and reopen where the
/// user was standing when it is not a save this intent accepts.
///
/// Contract 7: the OS dialog's filter is display-only, so the returned path must clear the same gate
/// the in-game LISTING applies -- rejecting is the OS-mode analogue of "the file simply is not
/// listed", which is why there is no error UI. The reopened dialog IS the feedback.
///
/// `stage` runs on the accepted path WHILE THE DIALOG CLAIM IS STILL HELD, and that ordering is
/// load-bearing rather than incidental. The save-flow tick runs concurrently and reads
/// `SAVE_PICKER_OS_DIALOG_OPEN` as its "a browser is live" term; if the claim dropped first, a tick
/// landing in the gap would see no dialog, no browser and no latch and end the flow as abandoned
/// before the caller could stage anything. Taking the staging as a closure makes that window
/// impossible to open by accident.
///
/// Returns `Staged(stage(path))`, or `Dismissed` for cancel / comdlg32 failure / reopen exhaustion
/// -- in which case `stage` never ran and nothing was staged, which is exactly what stage 3 already
/// reads as "the user abandoned the save". `NotOpened` is the third case and is deliberately NOT
/// spelled the same as a dismissal: no dialog ran, so the caller's open request is still owed.
fn os_pick_validated<T>(
    save_as: bool,
    mut start_dir: String,
    leaf: &str,
    extensions: &[&str],
    intent: &crate::experiments::save_picker::PickerIntent,
    stage: impl FnOnce(&str) -> T,
) -> OsPickResult<T> {
    // H4: refuse while the core CreateFileW detour is still settling. Installing a MinHook suspends
    // every other thread and allocates while they are frozen, and a thread parked in comdlg32
    // holding a heap or shell critical section is the one deadlock candidate. Every installer in
    // this DLL is attach-time and long finished before a user reaches System>Quit, so this gate
    // removes the overlap rather than reasoning about it. NAMED ACCEPTANCE: any FUTURE lazy MinHook
    // install reachable from in-world reopens this hazard.
    if !crate::experiments::save_file_core_hooks_live() {
        append_autoload_debug(format_args!(
            "save-picker-os: refusing to open -- the core CreateFileW detour is not live yet, and installing a hook while a thread is parked in comdlg32 can deadlock"
        ));
        return OsPickResult::NotOpened;
    }
    let Some(_claim) = OsDialogClaim::claim() else {
        return OsPickResult::NotOpened;
    };
    // COVER THE GAME FOR EXACTLY AS LONG AS IT IS FROZEN. Everything below this line runs with the
    // menu thread parked inside comdlg32, so the game renders nothing and a user with no cover sees
    // a still frame that is indistinguishable from a hang. `_dim` is declared AFTER `_claim`, so it
    // drops FIRST and the screen is released the instant the dialog is gone, while the claim still
    // covers the staging closure below.
    //
    // The bracket is the whole `os_pick_validated`, not each `os_dialog_run`, ON PURPOSE: an invalid
    // pick REOPENS the dialog, and a per-call bracket would flash the game back at full brightness
    // between the two dialogs. From the user's side the reopen is one continuous "pick a save",
    // which is what the cover should track.
    let _dim = picker_dim_arm(if save_as { "save-as" } else { "load" });
    let commit_window_armed = save_dest_commit_window_armed();
    let mut attempts = 0usize;
    loop {
        let outcome = os_dialog_run(save_as, &start_dir, leaf, extensions, commit_window_armed);
        let picked = match &outcome {
            OsPickOutcome::Picked(path) => path.clone(),
            OsPickOutcome::Cancelled | OsPickOutcome::Failed { .. } => {
                return OsPickResult::Dismissed;
            }
        };
        let verdict =
            crate::experiments::save_picker::save_picker_accepts(
                Path::new(&picked),
                intent,
                extensions,
            );
        let Err(reason) = verdict else {
            // `_claim` outlives this expression and drops on return, so every latch `stage` sets is
            // visible to the tick before the dialog term clears.
            return OsPickResult::Staged(stage(&picked));
        };
        SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_OS_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_OS_LAST_REJECT_REASON.store(reason as usize, Ordering::SeqCst);
        attempts += 1;
        append_autoload_debug(format_args!(
            "save-picker-os: rejected '{}' -- {reason:?} (reason={}); reopening ({attempts}/{SAVE_PICKER_OS_MAX_REOPENS})",
            system_quit_windows_path_for_log(&picked),
            reason as usize
        ));
        if !should_reopen(&outcome, false, attempts - 1) {
            SAVE_PICKER_OS_REOPEN_EXHAUSTED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-os: {SAVE_PICKER_OS_MAX_REOPENS} consecutive invalid picks -- giving up and taking the cancel path (a comdlg32 that fails instantly must not spin the menu pump)"
            ));
            return OsPickResult::Dismissed;
        }
        SAVE_PICKER_OS_REOPEN_COUNT.fetch_add(1, Ordering::SeqCst);
        // Reopen where they were, not back at the start.
        if let Some(parent) = Path::new(&picked)
            .parent()
            .and_then(|parent| parent.to_str())
            .filter(|parent| !parent.is_empty())
        {
            start_dir = parent.to_owned();
        }
    }
}

/// OS-mode LOAD source: pick a save container, then hand it to the unchanged in-game ingest
/// pipeline and stage the slot view exactly as the in-game pick does.
///
/// `SAVE_PICKER_SYSTEM_DIALOG` is stored from the row's action object BEFORE the dialog opens, the
/// same way the in-game arm does it: the menu-pump resubmit reopens `05_010` through that dialog and
/// abandons the reopen if it is 0. `SYSTEM_QUIT_PROFILE_SELECT_WINDOW` is already 0 in OS mode
/// (there is no picker window), so the resubmit's precondition holds with no window to close.
pub(crate) unsafe fn os_open_save_picker_load(action_obj: usize) -> PickerOpenOutcome {
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-os: refused to open -- {reason}"
            ));
            return PickerOpenOutcome::NotOpened;
        }
    };
    unsafe { system_quit_save_swap_restore_profile_summary("save-picker-os-reopen") };
    if !system_quit_save_swap_arm_original(&save_path) {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        return PickerOpenOutcome::NotOpened;
    }
    let Some(start_dir) = save_picker_start_dir().and_then(|dir| dir.to_str().map(str::to_owned))
    else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-os: refused to open -- no readable start directory (preferred/save-dir/default-root all unavailable)"
        ));
        return PickerOpenOutcome::NotOpened;
    };
    // Same flavor filter and the same source of it as the in-game picker and the ingest pipeline.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-os-picker-open");
    let extensions: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    // Store the owning System dialog BEFORE the blocking call: the menu-pump resubmit needs it, and
    // after the dialog returns the action object may no longer be the identity that survives.
    SAVE_PICKER_ACTION_OBJ.store(action_obj, Ordering::SeqCst);
    SAVE_PICKER_SYSTEM_DIALOG.store(
        unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(0),
        Ordering::SeqCst,
    );
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    let staged = os_pick_validated(
        false,
        start_dir,
        "",
        extensions,
        &crate::experiments::save_picker::PickerIntent::LoadSource,
        |picked| {
            // The SECOND gate, unchanged: BND4 parse, SteamID normalization, ProfileSummary
            // preview, candidate staging, picked-dir memory. The predicate above only added the
            // listing gate the dialog bypassed; nothing here is weakened.
            if !unsafe { system_quit_ingest_picked_save(picked) } {
                SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-picker-os: ingest refused '{}' after the listing predicate accepted it; nothing staged",
                    system_quit_windows_path_for_log(picked)
                ));
                return false;
            }
            SAVE_PICKER_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
            // Hand off to the SAME menu-pump resubmit the in-game pick uses, which reopens `05_010`
            // as the normal slot view. Contract 5: the slot view is always ours.
            SAVE_PICKER_OPEN_SLOTS_PENDING.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-os: accepted '{}'; slot view staged for the menu pump (dialog=0x{:x})",
                system_quit_windows_path_for_log(picked),
                SAVE_PICKER_SYSTEM_DIALOG.load(Ordering::SeqCst)
            ));
            true
        },
    );
    match staged {
        OsPickResult::Staged(true) => PickerOpenOutcome::Opened,
        // Nothing staged, the System menu untouched. Restore the preview we armed above so the
        // user's real rows are what the System UI shows. There is no retry latch on this surface --
        // the row press IS the request -- so a cancel here simply leaves the user standing on the
        // System>Quit rows, which is the Back semantics the save-destination surface now matches.
        other => {
            unsafe { system_quit_save_swap_restore_profile_summary("save-picker-os-no-pick") };
            match other {
                // No dialog ever appeared, so this is NOT a user decision. It used to be counted as
                // a cancel (every `None` was); now that the two are distinguishable, counting a
                // refusal as a user's Cancel is just a telemetry lie.
                OsPickResult::NotOpened => PickerOpenOutcome::NotOpened,
                OsPickResult::Dismissed => {
                    SAVE_PICKER_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
                    PickerOpenOutcome::Dismissed
                }
                // The ingest refused a path the listing predicate had accepted: a dialog RAN and
                // came back, so the request is discharged -- but that is OUR refusal, not the
                // user's, and `SAVE_PICKER_PICK_REJECT_COUNT` already counted it.
                OsPickResult::Staged(_) => PickerOpenOutcome::Dismissed,
            }
        }
    }
}

/// OS-mode SAVE DESTINATION: the Save-As dialog IS the destination browser.
///
/// Menu-pump owned, exactly like the in-game destination open: called from
/// `system_quit_menu_window_run_post` after the save-flow tick stages
/// `SAVE_DEST_OPEN_PICKER_PENDING`.
///
/// Three things this deliberately does NOT do, each of which would be a bug:
///
///  * it never calls `save_picker_native_close`. There is no picker window in OS mode, and handing
///    the System dialog to that helper would dispatch the MenuWindow cancel-close vfunc on a
///    `PropertyEditDialog` -- the exact mistake `system_quit_dialog_handlers` warns about. It stages
///    `SAVE_DEST_COMMIT_PENDING` and stops; stage 3 then sees no picker window and proceeds.
///  * it never writes `SAVE_FLOW_STAGE`. The in-game arm does, from the menu thread, bypassing
///    `save_flow_enter_stage` -- a filed defect (bd `er-effects-rs-8tq4` item 15). Adding a second
///    instance of a known defect is not "keeping the modes symmetric". It sets
///    `SAVE_DEST_CONFIRM_PENDING` and the TICK performs the transition.
///  * it stages nothing at all on cancel. Dropping the dialog claim with no latch set is precisely
///    what stage 3 reads as "the user abandoned the save". That was TRUE of the latches and FALSE
///    of the request: the menu pump's `SAVE_DEST_OPEN_PICKER_PENDING` stayed armed through the
///    cancel and reopened the dialog on the next pump, ~57 ms later, forever (bd
///    `er-effects-rs-rsxi`). The cancel path still needs no latch of its own -- what it needs is to
///    say `Dismissed` rather than "the open failed", which is what discharges that request.
pub(crate) unsafe fn os_open_save_dest_picker(system_dialog: usize) -> PickerOpenOutcome {
    const HEAP_LO: usize = 0x10000;
    if system_dialog < HEAP_LO || system_dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-picker-os: refused save-as -- System dialog=0x{system_dialog:x} is not heap-like"
        ));
        return PickerOpenOutcome::NotOpened;
    }
    // The SAME start-dir/leaf resolution the in-game destination browser uses, so the two modes
    // cannot open in different places.
    let Some((start_dir, loaded_file_name)) = save_dest_start_dir() else {
        return PickerOpenOutcome::NotOpened;
    };
    let Some(start_dir) = start_dir.to_str().map(str::to_owned) else {
        append_autoload_debug(format_args!(
            "save-picker-os: refused save-as -- the loaded save's folder is not representable as text"
        ));
        return PickerOpenOutcome::NotOpened;
    };
    let seamless = save_picker_seamless_mode_after_settle("system-quit-os-save-dest-picker-open");
    let extensions: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    let intent = crate::experiments::save_picker::PickerIntent::SaveDestination {
        loaded_file_name: loaded_file_name.clone(),
    };
    SAVE_DEST_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    let staged = os_pick_validated(
        true,
        start_dir,
        &loaded_file_name,
        extensions,
        &intent,
        |picked| {
            let target = PathBuf::from(picked);
            // The SAME mode-free routing decision the in-game browser's activation makes, so the
            // overwrite gate cannot differ between surfaces.
            match save_dest_route_picked_target(&target) {
                DestRoute::ConfirmOverwrite => {
                    SAVE_DEST_TARGET_EXISTING_COUNT.fetch_add(1, Ordering::SeqCst);
                    save_dest_set_target(target, "os-save-as");
                    SAVE_DEST_CONFIRM_PENDING.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "save-picker-os: save-as chose the existing '{}'; the overwrite confirm is owed to the tick",
                        system_quit_windows_path_for_log(picked)
                    ));
                }
                DestRoute::CommitDirect => {
                    SAVE_DEST_TARGET_NEW_COUNT.fetch_add(1, Ordering::SeqCst);
                    save_dest_set_target(target, "os-save-as");
                    SAVE_DEST_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
                    save_flow_box_clear();
                    SAVE_DEST_COMMIT_PENDING.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "save-picker-os: save-as named the new '{}'; commit staged, no window to close",
                        system_quit_windows_path_for_log(picked)
                    ));
                }
            }
        },
    );
    match staged {
        OsPickResult::Staged(()) => PickerOpenOutcome::Opened,
        OsPickResult::Dismissed => {
            append_autoload_debug(format_args!(
                "save-picker-os: save-as closed without choosing; nothing staged and the menu pump's open request is DISCHARGED (no reopen) -- the save-flow tick will end the flow with nothing written"
            ));
            PickerOpenOutcome::Dismissed
        }
        OsPickResult::NotOpened => PickerOpenOutcome::NotOpened,
    }
}

#[cfg(test)]
mod save_picker_os_dialog_tests {
    use super::*;

    /// A `FALSE` return means EITHER cancel OR failure, and the extended error is the only thing
    /// that tells them apart. Collapsing them would make a broken comdlg32 look like a user
    /// decision -- and only one of the two is a bug of ours.
    #[test]
    fn a_cancel_and_a_comdlg32_failure_are_not_the_same_outcome() {
        assert_eq!(
            classify_os_outcome(false, 0, None),
            OsPickOutcome::Cancelled
        );
        assert_eq!(
            classify_os_outcome(false, 0, Some("Z:\\stale.sl2".to_owned())),
            OsPickOutcome::Cancelled,
            "a stale buffer left over from a previous open is not a pick"
        );
        assert_eq!(
            classify_os_outcome(false, 0x3002, None),
            OsPickOutcome::Failed { error: 0x3002 }
        );
        assert_eq!(
            classify_os_outcome(true, 0, Some("Z:\\saves\\ER0000.sl2".to_owned())),
            OsPickOutcome::Picked("Z:\\saves\\ER0000.sl2".to_owned())
        );
        assert_eq!(
            classify_os_outcome(true, 0, None),
            OsPickOutcome::Failed { error: 0 },
            "TRUE with an empty buffer is a dialog that lied, not a pick"
        );
        assert_eq!(
            classify_os_outcome(true, 0, Some(String::new())),
            OsPickOutcome::Failed { error: 0 }
        );
    }

    /// ONLY an invalid pick reopens, and only under the bound. A user who keeps cancelling must
    /// never be re-prompted, and a comdlg32 that fails instantly must not be able to spin the loop
    /// on the thread that owns the menu pump.
    #[test]
    fn only_an_invalid_pick_reopens_and_the_bound_is_finite() {
        let picked = OsPickOutcome::Picked("Z:\\saves\\ER0000.sl2".to_owned());
        for attempts in 0..SAVE_PICKER_OS_MAX_REOPENS {
            assert!(
                should_reopen(&picked, false, attempts),
                "an invalid pick at attempt {attempts} must reopen"
            );
        }
        assert!(
            !should_reopen(&picked, false, SAVE_PICKER_OS_MAX_REOPENS),
            "the reopen bound must be finite"
        );
        assert!(
            !should_reopen(&picked, true, 0),
            "a VALID pick is the end of the loop, not a reopen"
        );
        for attempts in [0, 1, SAVE_PICKER_OS_MAX_REOPENS] {
            assert!(
                !should_reopen(&OsPickOutcome::Cancelled, false, attempts),
                "a cancel must never reopen"
            );
            assert!(
                !should_reopen(&OsPickOutcome::Failed { error: 0x3002 }, false, attempts),
                "a comdlg32 failure must never reopen"
            );
        }
    }

    /// The filter is a double-NUL-terminated pair of description/pattern strings. Malformed
    /// termination is the classic comdlg32 crash, and it is invisible until a dialog opens.
    #[test]
    fn the_filter_is_a_double_nul_terminated_description_pattern_pair() {
        let filter = os_dialog_filter(&["co2", "sl2"]);
        assert_eq!(
            filter.last().copied(),
            Some(0),
            "the filter must end in a NUL"
        );
        assert_eq!(
            filter[filter.len() - 2],
            0,
            "the filter must be DOUBLE-NUL terminated"
        );
        let text = String::from_utf16(&filter).expect("the filter is valid UTF-16");
        let fields: Vec<&str> = text.trim_end_matches('\0').split('\0').collect();
        assert_eq!(fields.len(), 2, "description then pattern, nothing else");
        assert_eq!(fields[0], "Elden Ring save (*.co2;*.sl2)");
        assert_eq!(fields[1], "*.co2;*.sl2");
        let vanilla = String::from_utf16(&os_dialog_filter(&["sl2"])).expect("valid UTF-16");
        assert!(vanilla.contains("*.sl2") && !vanilla.contains("co2"));
    }

    /// A path is read up to its NUL, and an empty buffer yields nothing rather than an empty path
    /// that would later read as a pick.
    #[test]
    fn a_pick_is_decoded_up_to_its_nul() {
        let mut buffer = [0u16; 32];
        for (index, unit) in "Z:\\saves\\ER0000.sl2".encode_utf16().enumerate() {
            buffer[index] = unit;
        }
        assert_eq!(
            os_pick_path_from_buffer(&buffer).as_deref(),
            Some("Z:\\saves\\ER0000.sl2")
        );
        assert_eq!(os_pick_path_from_buffer(&[0u16; 8]), None);
        assert_eq!(os_pick_path_from_buffer(&[]), None);
    }
}
