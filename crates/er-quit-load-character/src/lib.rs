//! Standalone ME3 shell for the two System>Quit character rows, with no product DLL in the
//! profile.
//!
//! Each row is a cell of the same derived six-cell Quit grid `er-quit-menu` stands on, cloned by
//! the same `er_quit_menu_core::row_cloner`. The difference between the two shells is one argument:
//! this one passes a [`RowSet`](er_quit_menu_core::row_cloner::RowSet) carrying the character pair
//! and supplies the flows those rows need. A row that is not in the set is never cloned, so a press
//! that could reach a flow this shell does not have never happens.
//!
//! # What the rows do
//!
//! **Load Character** submits the game's own `05_010_ProfileSelect` window over the System dialog
//! the press came from -- the character picker the title screen uses, opened in-world. Everything
//! after that is the game's: the cursor, the list, the confirm box and the load the confirm arms.
//! This shell installs nothing on any of it. **Load Character from File** opens the browse picker
//! in `er_quit_menu_core::save_picker_menu` and hands the pick to the same window.
//!
//! That is the whole difference from the product. `er-quickload` detours
//! `CS::ProfileLoadDialog::load_activate` and turns a pick into its own save-safe switch -- return
//! to the title, tear the world down, reload the picked slot -- because it has a return-title
//! chain, a title-time continue driver and an autoload phase machine in flight, and the native
//! in-world load collides with them. None of that exists here, so the native chain is left to run.
//! **Whether it completes has not been observed**; see the pull request for what is and is not
//! proven.
//!
//! # The second character row, and what it still does not carry
//!
//! **Load Character from File** browses and opens, since the picker moved to
//! `er_quit_menu_core::save_picker_menu` on 2026-09-11. What a standalone shell does not install is
//! the product's nine picker steps -- the layout editor's font heights, the save-swap ledger's row
//! records, the save-flow box, the intent router and the three passive nav-input reads. Each has a
//! neutral default, so the surface is the game's own chrome rather than the product's, and a held
//! direction is never seen: the edge-scroll and the drive strip answer only to activations.
//!
//! # Never in the same profile as the product, or as `er-quit-menu`
//!
//! All three derive the same six-cell `02_040_optionsetting` grid, and
//! `er_gfx::options_02_040::quit6` fail-closes when its input is not vanilla -- so a second deriver
//! handed already-derived bytes correctly refuses, and co-loading cannot produce a working tab.
//! `scripts/me3-dll-conflicts.toml` records both pairs as duplicate owners, and the profile
//! generator refuses to emit a profile carrying two of them.
//!
//! # What stays refused here
//!
//! Every product-owned answer in the host seam stays at its neutral default -- including the
//! save-write bypass, so a product-less load can never push a write past `er-save-suppress`.

// A cdylib whose every consumer is `DllMain` and the hooks it installs, all of them
// `#[cfg(windows)]`. On a host build the shell is compiled with its only callers cfg'd
// out, so `dead_code`/`unused_imports` there report the cfg, not real debt. The shipping
// target (x86_64-pc-windows-msvc) carries the full deny with no allows.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-quit-load-character.log";

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

/// Where the standalone log lands: next to the executable, falling back to the CWD.
fn log_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Fresh per process: the first line of a run truncates the file (rotating the previous
/// run's aside as `.log.prev`), later lines append. No log in this repo accumulates across
/// runs -- mixing evidence from builds that no longer exist is how a count over one file
/// gets read as one run's behaviour.
fn append_log(dir: &Path, args: std::fmt::Arguments<'_>) {
    er_game_base::log::append_line(
        &dir.join(LOG_FILE_NAME),
        format_args!("er-quit-load-character: {args}"),
    );
}

/// The standalone host seam: this DLL has no product behind it, so every product-owned
/// answer stays at its neutral default -- including the save-write bypass, which must stay
/// refused so a product-less load can never push a write past `er-save-suppress`.
fn install_standalone_host() {
    let _ = er_quit_menu_core::install_host(er_quit_menu_core::QuitMenuHost {
        append_autoload_debug: standalone_log,
        append_crash_log: standalone_log,
        ..er_quit_menu_core::QuitMenuHost::defaults()
    });
}

fn standalone_log(args: std::fmt::Arguments<'_>) {
    append_log(&log_dir(), args);
}

/// The two rows this shell arms. Spelled out rather than reached for as a named constant, because
/// the set and the action table below have to agree field for field: a row here with no flow beside
/// it is a row that appears and does nothing.
///
/// **Load Character from File** joined it once the picker moved into `er-quit-menu-core`
/// (2026-09-11). Until then the browse surface, the row staging and the ingest were all
/// `pub(crate)` inside `er-quickload`, so a shell could name the row and had nothing to open.
#[cfg(windows)]
const CHARACTER_ROWS: er_quit_menu_core::row_cloner::RowSet =
    er_quit_menu_core::row_cloner::RowSet {
        load_character: true,
        load_character_from_file: true,
        ..er_quit_menu_core::row_cloner::RowSet::NONE
    };

/// What a press on each row reaches. Exactly one entry is filled, and it is the one row
/// [`CHARACTER_ROWS`] arms; the rest are flows no press can reach rather than flows this shell
/// is missing.
#[cfg(windows)]
fn row_actions() -> er_quit_menu_core::row_cloner::QuitRowActions {
    er_quit_menu_core::row_cloner::QuitRowActions {
        open_profile_load_dialog: Some(
            er_quit_menu_core::profile_load_dialog::system_quit_open_profile_load_dialog,
        ),
        open_save_picker_menu: Some(open_save_picker_for_row),
        ..er_quit_menu_core::row_cloner::QuitRowActions::default()
    }
}

/// Open the browse picker for a **Load Character from File** press.
///
/// The row router answers in booleans; the picker answers in outcomes, and the two disagree about
/// one case on purpose. `Dismissed` means a picker ran and the user backed out of it -- the press
/// was carried out, so the row must not re-arm -- while the router only needs to know whether the
/// press was taken. Collapsing them here rather than widening the router keeps that distinction
/// where the picker made it.
///
/// # Safety
///
/// Menu-thread press context, with `action_obj` the row's live action object.
#[cfg(windows)]
unsafe fn open_save_picker_for_row(action_obj: usize) -> bool {
    unsafe { er_quit_menu_core::save_picker_menu::system_quit_open_save_picker_menu(action_obj) }
        .request_discharged()
}

/// Arm both character rows. Runs on its own thread because the game-task registration waits for
/// the game's task manager to exist, and waiting inside the loader lock deadlocks the process.
#[cfg(windows)]
fn arm_load_character_row() {
    // Safety: a bootstrap thread, once per process (`START` gates the spawn), before the Quit tab
    // has built a dialog.
    let arm = unsafe { er_quit_menu_core::arm::arm_standalone(CHARACTER_ROWS, row_actions()) };
    if !arm.is_complete() {
        append_log(
            &log_dir(),
            format_args!(
                "some of the row's machinery did not install: {arm:?}; the row may be absent or inert"
            ),
        );
    }
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // First, before anything that can panic. A panic in a cdylib crosses an
        // `extern "system"` boundary and becomes an abort, which does not dispatch to a
        // vectored handler -- so `er_crash_logging` writes no record at all and the process
        // just vanishes. This hook is what turns that silence into a file:line. The hook is
        // per-DLL: every cdylib links its own `er-game-base`, so another shell installing it
        // does nothing here. Enforced by `scripts/check-panic-reporter-installed.py`.
        er_game_base::panic_report::report_panics_to("er-quit-load-character", standalone_log);

        let module_base = module as usize;
        START.call_once(|| {
            // Before the thread, so no moved code can run against an un-installed seam.
            install_standalone_host();
            append_log(
                &log_dir(),
                format_args!(
                    "loaded module_base=0x{module_base:x}; arming the System>Quit Load Character row"
                ),
            );
            std::thread::spawn(arm_load_character_row);
        });
    }
    DLL_MAIN_SUCCESS
}

// Windows-only because the row set and the action table these assert on live in
// `er_quit_menu_core::row_cloner`, which is itself `#[cfg(windows)]`. They run under wine from the
// `cargo xwin test --lib` list in `scripts/check-rust-build.sh`, which is where every other
// windows-only unit test in this workspace runs.
#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_standalone_host_installs_exactly_once() {
        assert!(install_host_once());
        assert!(!install_host_once());
    }

    fn install_host_once() -> bool {
        er_quit_menu_core::install_host(er_quit_menu_core::QuitMenuHost {
            append_autoload_debug: standalone_log,
            append_crash_log: standalone_log,
            ..er_quit_menu_core::QuitMenuHost::defaults()
        })
    }

    /// The row set and the action table have to agree field for field. An armed row with no
    /// action is a row that appears and does nothing when pressed; an action beside a row that was
    /// never armed is a pointer to a flow no press can reach, which is dead weight rather than a
    /// defect but still means the two drifted.
    ///
    /// The pairing is asserted rather than each half separately, because each half on its own is
    /// the thing the other is supposed to catch.
    #[test]
    fn every_armed_row_has_a_flow_and_no_unarmed_row_carries_one() {
        let rows = CHARACTER_ROWS;
        let actions = row_actions();
        assert_eq!(
            actions.open_profile_load_dialog.is_some(),
            rows.load_character,
            "Load Character"
        );
        assert_eq!(
            actions.open_save_picker_menu.is_some(),
            rows.load_character_from_file,
            "Load Character from File"
        );
        // The Save Game pair and the row-table reset belong to rows this shell never arms, and the
        // drive-strip note belongs to the save picker's browse surface it does not have.
        assert!(actions.save_game_start_flow.is_none());
        assert!(actions.save_game_request_save_only.is_none());
        assert!(actions.row_table_reset.is_none());
        assert!(actions.note_drive_strip_click_event.is_none());
    }

    /// Both character rows, and neither build row. The build pair belongs to the sibling shell
    /// `er-quit-menu`, and a shell arming both halves of the tab would be the co-loading the
    /// conflict table exists to refuse, written into one DLL instead.
    #[test]
    fn this_shell_arms_the_character_switch_and_neither_build_row() {
        let rows = CHARACTER_ROWS;
        let armed: Vec<&str> = [
            ("Load Character", rows.load_character),
            ("Load Character from File", rows.load_character_from_file),
            ("Load Build from URL", rows.load_build_from_url),
            ("Generate Build Link", rows.generate_build_link),
        ]
        .into_iter()
        .filter_map(|(label, armed)| armed.then_some(label))
        .collect();
        assert_eq!(armed, vec!["Load Character", "Load Character from File"]);
    }
}
