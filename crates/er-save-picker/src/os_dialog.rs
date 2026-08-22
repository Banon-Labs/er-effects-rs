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
// THAT WARNING IS ABOUT THE THREAD, NOT ABOUT WHICH WINDOW OWNS THE DIALOG, and the two got read as
// one thing once already. `hwndOwner` is now the DIM COVER rather than the game window whenever a
// cover is up (see `os_dialog_owner` for the full argument): the call is still inline on this same
// thread, so nothing above changes, and what comdlg32 disables becomes a click-through,
// non-activating window with no input to lose. The modality still comes from the block -- from the
// sentence directly above this one -- and never came from `EnableWindow`.
//
// The game task keeps ticking meanwhile -- see `save_flow_next_stage_ticks`, which is why the
// flow's deadlines are frozen while `SAVE_PICKER_OS_DIALOG_OPEN` is set.
//
// THE BOOT INTENT IS THE EXCEPTION, and it is an exception about WHICH THREAD, never about this
// file's contract. See `save_picker_boot.rs`: at a missing-save boot the only threads that reach
// the picker are the D3D12 Present hook and the CSTaskImp recurring task, and blocking either one
// stalls the game's own frame loop rather than a menu pump we are trying to make modal. That arm
// therefore calls `os_pick_validated` from a thread WE own and passes `no_picker_cover`, because
// with no game thread blocked Present keeps running and the boot's own overlay keeps drawing --
// there is nothing frozen for a cover to explain.

// The comdlg32/user32 surfaces below are `#[cfg(windows)]`, so a HOST build compiles their
// helpers, constants and counter imports with every caller cfg'd out. `dead_code` /
// `unused_imports` there describe the cfg, not real debt; the SHIPPING target
// (x86_64-pc-windows-msvc) carries the full deny with no allows.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::{path::Path, sync::atomic::Ordering};

#[cfg(windows)]
use std::{ffi::c_void, time::Instant};

#[cfg(windows)]
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Controls::Dialogs::{
            CommDlgExtendedError, GetOpenFileNameW, GetSaveFileNameW, OFN_DONTADDTORECENT,
            OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR,
            OFN_NOTESTFILECREATE, OFN_PATHMUSTEXIST, OPEN_FILENAME_FLAGS, OPENFILENAMEW,
        },
        WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW},
    },
};
#[cfg(windows)]
use windows::core::PCWSTR;

use crate::{
    host::{
        PickerCover, PickerCoverFactory, append_autoload_debug, game_main_window,
        save_dest_commit_window_armed, save_file_core_hooks_live, system_quit_windows_path_for_log,
    },
    model::{PickerIntent, PickerStatusMessage, save_picker_accepts},
};

use er_telemetry::counters::SAVE_PICKER_OS_CANCEL_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_CLOSED_WITH_PATH;
use er_telemetry::counters::SAVE_PICKER_OS_DIALOG_OPEN;
use er_telemetry::counters::SAVE_PICKER_OS_ERROR_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_LAST_ERROR;
use er_telemetry::counters::SAVE_PICKER_OS_LAST_REJECT_REASON;
use er_telemetry::counters::SAVE_PICKER_OS_OPEN_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_OWNER_HWND;
use er_telemetry::counters::SAVE_PICKER_OS_OWNER_IS_COVER;
use er_telemetry::counters::SAVE_PICKER_OS_REJECT_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_REOPEN_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_REOPEN_EXHAUSTED;
use er_telemetry::counters::SAVE_PICKER_OS_SAVELIKE_OPENS;
use er_telemetry::counters::SAVE_PICKER_OS_TICKS_FROZEN;
use er_telemetry::counters::SAVE_PICKER_PICK_REJECT_COUNT;

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
pub enum OsPickOutcome {
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

fn extension_label(extensions: &[&str]) -> String {
    extensions
        .iter()
        .map(|ext| ext.trim_start_matches('.').to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("/.")
}

#[cfg(windows)]
fn show_os_reject_message(cover: Option<&PickerCover>, message: &PickerStatusMessage) {
    let body = format!("{}\n\n{}", message.headline(), message.detail());
    let body_wide: Vec<u16> = body.encode_utf16().chain(core::iter::once(0)).collect();
    let title_wide: Vec<u16> = "Save rejected"
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
    let owner = os_dialog_owner(cover);
    let hwnd = (owner != 0).then_some(HWND(owner as *mut c_void));
    let _ = unsafe {
        MessageBoxW(
            hwnd,
            PCWSTR(body_wide.as_ptr()),
            PCWSTR(title_wide.as_ptr()),
            MB_OK | MB_ICONWARNING,
        )
    };
}

#[cfg(not(windows))]
fn show_os_reject_message(_cover: Option<&PickerCover>, _message: &PickerStatusMessage) {}

/// Why an open ended with nothing staged. THREE reasons, not one, and every collapse between them
/// has already shipped as a bug.
///
/// A `bool`/`Option` return conflates all three, and two different consumers were burnt by two
/// different halves of that conflation:
///
///  * conflating "a dialog RAN and answered" with "no dialog ran" is the reopen loop PR #107 had to
///    unpick one level up (a `bool` that meant both "the picker ran" and "the picker is still up",
///    so the menu pump re-armed a cancelled dialog every ~57 ms, forever -- bd
///    `er-effects-rs-rsxi`). That is `Cancelled`/`Failed` vs `NotOpened`.
///  * conflating "the user decided" with "we could not ask" is what would let the BOOT arm quit a
///    user's game over a defect in comdlg32, because at a missing-save boot a Cancel is
///    `ExitProcess(0)`. That is `Cancelled` vs `Failed`.
///
/// The two System>Quit arms discriminate only the first split, and say so where they map these onto
/// [`PickerOpenOutcome`]. The boot arm is the one caller that needs both, and `boot_abort_action`
/// is where it makes the decision -- pure, and therefore pinned by a test rather than by a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsPickAbort {
    /// The user dismissed a dialog that RAN. A DECISION, and the only outcome a caller may treat as
    /// one. Terminal: the request that asked for the dialog has been carried out.
    Cancelled,
    /// A dialog RAN and came back unusable: comdlg32 failed, or the invalid-pick reopen bound was
    /// exhausted. Terminal for the same reason `Cancelled` is -- a dialog happened, so re-asking
    /// would reopen it -- but NEVER a user decision, so no caller may act on it as a choice. A
    /// caller with a second surface should use that surface instead.
    Failed,
    /// NO dialog ran at all: the core `CreateFileW` detour is not live yet, or a re-entrant open was
    /// refused because one is already up. The request is STILL OWED, and a caller that can ask again
    /// on its next tick must.
    NotOpened,
}

/// What one [`os_pick_validated`] call did: `Ok(staged)`, or one of the three ways an open ends with
/// nothing staged.
///
/// The `Err` half used to be a single `None`, and collapsing it is what let a user's Cancel be
/// retried as though the dialog had never opened (bd `er-effects-rs-rsxi`). Those are opposite
/// facts: a dismissal means a dialog RAN and was answered, so the request that asked for it is
/// finished; a `NotOpened` means no dialog ran at all, so the request still stands.
pub type OsPickResult<T> = Result<T, OsPickAbort>;

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

/// The `hwndOwner` for a dialog: THE DIM COVER when one is up, otherwise the game's own main
/// window. Never `hooks::own_window()` -- see [`game_main_window`].
///
/// WHY THE COVER (user report 2026-07-31: the picker came up BEHIND the blur). `hwndOwner` is not
/// a hint, it is the z-order relation: a window is always above the window that owns it. Owning the
/// dialog to the game window only promises dialog-above-GAME, and left dialog-vs-cover to be
/// settled by creation order -- a race the cover can and did win, because its `HWND_TOP` raise was
/// issued asynchronously by the overlay thread and could land after comdlg32's window existed. The
/// cover is itself an owned popup of the ER window (`picker_dim::attach_to_game`), so owning the
/// dialog to the cover makes the whole chain game < cover < dialog a window-manager invariant.
///
/// THE FILE-HEADER INPUT WARNING DOES NOT APPLY TO THIS, and the distinction is worth being exact
/// about. That warning is about calling the dialog from ANOTHER THREAD while the game window is the
/// owner: comdlg32 disables its owner, so a cross-thread open would disable the game window while
/// the thread that polls raw input kept running. Nothing about the thread changes here -- the call
/// is still inline on the thread that owns the pump. What changes is WHICH window comdlg32
/// disables, and the cover is `WS_EX_NOACTIVATE | WS_EX_TRANSPARENT` behind a bare `DefWindowProcW`
/// proc, so it has no input to be deprived of.
///
/// Leaving the game window ENABLED is likewise not a loss of modality. The modality has never come
/// from `EnableWindow` -- the header says so directly: "Blocking the menu pump means the menus
/// cannot process anything at all, which is the modality we want." The one thing an enabled game
/// window could still do that a disabled one could not -- be clicked, and raised over the dialog --
/// is precisely what the ownership chain forbids, since a window can never be raised above the
/// windows it owns.
///
/// A null result is NOT fatal: pass it through and log `owner=none`, so a report can distinguish
/// "we passed no owner" from "we passed one and the dialog still went behind the game".
fn os_dialog_owner(cover: Option<&PickerCover>) -> usize {
    // Null cover = no cover to own to: the missing-save BOOT arm passes `no_picker_cover` and
    // raises none, and an arm whose cover did not come up in time reports null rather than hand
    // comdlg32 a window that is still 1x1 at the origin -- comdlg32 CENTRES the dialog on its
    // owner, so that would put the picker in the desktop's top-left corner.
    let cover_hwnd = cover.map(PickerCover::owner_hwnd).unwrap_or(0);
    let is_cover = cover_hwnd != 0;
    let hwnd = if is_cover {
        cover_hwnd
    } else {
        game_main_window()
    };
    SAVE_PICKER_OS_OWNER_HWND.store(hwnd, Ordering::SeqCst);
    SAVE_PICKER_OS_OWNER_IS_COVER.store(usize::from(is_cover), Ordering::SeqCst);
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
#[cfg(windows)]
fn os_dialog_run(
    save_as: bool,
    start_dir: &str,
    leaf: &str,
    extensions: &[&str],
    commit_window_armed: bool,
    cover: Option<&PickerCover>,
) -> OsPickOutcome {
    let filter = os_dialog_filter(extensions);
    let title: Vec<u16> = if save_as {
        "Save Game to..."
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect()
    } else {
        // Same words as the row that opens it, so the window the user context-switches into is
        // recognisably the thing they pressed.
        "Load Character from File"
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect()
    };
    let initial_dir: Vec<u16> = start_dir
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
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
    let owner = os_dialog_owner(cover);
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut c_void),
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
        owner,
        // Spell out the OWNERSHIP CHAIN, because "the picker is in front of the blur" is now a
        // structural claim about these three handles and a log that only shows one of them cannot
        // be used to check it. `owner=<cover>` with the cover owned by the game is the good shape;
        // `owner=<game>` while a cover is armed means the handshake timed out and this open's
        // stacking is back to a race.
        if owner == 0 {
            " (owner=none)".to_owned()
        } else if SAVE_PICKER_OS_OWNER_IS_COVER.load(Ordering::SeqCst) == 1 {
            format!(
                " (owner=the dim COVER, itself owned by ER window 0x{:x}: attach={} readback=0x{:x}; game < cover < dialog)",
                er_telemetry::counters::SAVE_PICKER_DIM_GAME_HWND.load(Ordering::SeqCst),
                er_telemetry::counters::SAVE_PICKER_DIM_OWNER_SET.load(Ordering::SeqCst),
                er_telemetry::counters::SAVE_PICKER_DIM_OWNER_READBACK.load(Ordering::SeqCst),
            )
        } else {
            format!(
                " (owner=the GAME window; no cover was up for this open -- dim_armed={} arm_wait_timeouts={})",
                er_telemetry::counters::SAVE_PICKER_DIM_ARMED.load(Ordering::SeqCst),
                er_telemetry::counters::SAVE_PICKER_DIM_ARM_WAIT_TIMEOUTS.load(Ordering::SeqCst),
            )
        },
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

#[cfg(not(windows))]
fn os_dialog_run(
    _save_as: bool,
    _start_dir: &str,
    _leaf: &str,
    _extensions: &[&str],
    _commit_window_armed: bool,
    _cover: Option<&PickerCover>,
) -> OsPickOutcome {
    OsPickOutcome::Failed { error: 0 }
}

/// A no-cover factory for callers that block no game thread.
pub fn no_picker_cover(_surface: &str) -> Option<PickerCover> {
    None
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
/// Returns `Ok(stage(path))`, or one of the three [`OsPickAbort`]s -- in which case `stage` never
/// ran and nothing was staged, which is exactly what stage 3 already reads as "the user abandoned
/// the save". `NotOpened` is deliberately NOT spelled the same as a dismissal: no dialog ran, so the
/// caller's open request is still owed. `Cancelled` and `Failed` are both terminal and are
/// deliberately not spelled the same either: only one of them is a user's decision, and the boot arm
/// answers a user's decision by quitting the game.
pub fn os_pick_validated<T>(
    save_as: bool,
    mut start_dir: String,
    leaf: &str,
    extensions: &[&str],
    intent: &PickerIntent,
    cover_factory: PickerCoverFactory,
    stage: impl FnOnce(&str) -> T,
) -> OsPickResult<T> {
    // H4: refuse while the core CreateFileW detour is still settling. Installing a MinHook suspends
    // every other thread and allocates while they are frozen, and a thread parked in comdlg32
    // holding a heap or shell critical section is the one deadlock candidate. Every installer in
    // this DLL is attach-time and long finished before a user reaches System>Quit, so this gate
    // removes the overlap rather than reasoning about it. NAMED ACCEPTANCE: any FUTURE lazy MinHook
    // install reachable from in-world reopens this hazard.
    if !save_file_core_hooks_live() {
        append_autoload_debug(format_args!(
            "save-picker-os: refusing to open -- the core CreateFileW detour is not live yet, and installing a hook while a thread is parked in comdlg32 can deadlock"
        ));
        return Err(OsPickAbort::NotOpened);
    }
    let Some(_claim) = OsDialogClaim::claim() else {
        return Err(OsPickAbort::NotOpened);
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
    let surface = if save_as { "save-as" } else { "load" };
    let cover = cover_factory(surface);
    let commit_window_armed = save_dest_commit_window_armed();
    let mut attempts = 0usize;
    loop {
        let outcome = os_dialog_run(
            save_as,
            &start_dir,
            leaf,
            extensions,
            commit_window_armed,
            cover.as_ref(),
        );
        let picked = match &outcome {
            OsPickOutcome::Picked(path) => path.clone(),
            // BOTH are terminal -- a dialog ran, so the request is discharged and must not be
            // re-armed -- and they are still two answers, because only the first is the user's.
            OsPickOutcome::Cancelled => return Err(OsPickAbort::Cancelled),
            OsPickOutcome::Failed { .. } => return Err(OsPickAbort::Failed),
        };
        let verdict = save_picker_accepts(Path::new(&picked), intent, extensions);
        let Err(reason) = verdict else {
            // `_claim` outlives this expression and drops on return, so every latch `stage` sets is
            // visible to the tick before the dialog term clears.
            return Ok(stage(&picked));
        };
        let message = reason.status_message(&extension_label(extensions));
        show_os_reject_message(cover.as_ref(), &message);
        SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_OS_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_OS_LAST_REJECT_REASON.store(reason as usize, Ordering::SeqCst);
        attempts += 1;
        append_autoload_debug(format_args!(
            "save-picker-os: rejected '{}' -- {reason:?} (reason={}) visible='{}: {}'; reopening ({attempts}/{SAVE_PICKER_OS_MAX_REOPENS})",
            system_quit_windows_path_for_log(&picked),
            reason as usize,
            message.headline(),
            message.detail()
        ));
        if !should_reopen(&outcome, false, attempts - 1) {
            SAVE_PICKER_OS_REOPEN_EXHAUSTED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-os: {SAVE_PICKER_OS_MAX_REOPENS} consecutive invalid picks -- abandoning the open (a comdlg32 that fails instantly must not spin the calling thread)"
            ));
            // FAILED, not Cancelled -- and not NotOpened either. Dialogs DID run, so the request is
            // discharged and the System>Quit arms still read this as "nothing staged" and leave the
            // System menu alone. But exhaustion is a dialog we could not get a usable answer out of
            // -- most plausibly a comdlg32 returning instantly with a stale path -- so calling it a
            // user cancel would let the boot arm quit the game over a comdlg32 defect.
            return Err(OsPickAbort::Failed);
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
