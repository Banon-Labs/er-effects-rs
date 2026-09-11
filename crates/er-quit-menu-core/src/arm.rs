//! Arming the System>Quit rows from a host that has nothing else.
//!
//! [`crate::row_cloner::arm`] installs the cloner and the router, which is all a host needs when it
//! already owns the surrounding machinery -- the product does, and calls that directly. A
//! standalone shell owns none of it, and three further things have to exist or the rows are
//! decoration:
//!
//! 1. **The grid.** Vanilla `02_040_optionsetting` ships a Quit Game panel with two cells. The
//!    cloned rows are cells three through six, so without the derived six-cell movie the cloner
//!    appends rows into cells that do not exist.
//! 2. **The menu pump.** The link field builds and submits a native `CS::SoftwareKeyboardJob`, and
//!    its display objects are only valid while its own window runs. Both are `MenuWindowJob::Run`
//!    work.
//! 3. **The game task.** A row press latches a request; the import that satisfies it mutates the
//!    inventory and `PlayerGameData` and must run on `FrameBegin`.
//!
//! Each is installed by its own module and reported separately, because each fails differently and
//! a run has to be able to say which one was missing.
//!
//! # Only the machinery the row set actually uses
//!
//! Items 2 and 3 exist for the link field and the importer behind it, and both cost a real detour:
//! the pump claims `MenuWindowJob::Run`, and the task registers on `CSTaskImp`. A shell arming no
//! build row has no field to drive and nothing to import, so installing them would put two claims
//! on the process for work that can never be requested. They are therefore installed only when the
//! row set contains a build row, and reported as [`None`] -- not as a failure and not as a success
//! -- when it does not. The grid and the rows themselves are needed by every row set.

use core::sync::atomic::Ordering;

use crate::host::append_autoload_debug;
use crate::row_cloner::{ArmError, QuitRowActions, RowSet};

/// What a standalone arm managed to install. Every field false is a load that will show no rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandaloneArm {
    /// The six-cell Quit grid and the link field's movie are being served.
    pub gfx_served: bool,
    /// The row cloner and the row router are installed.
    pub rows_armed: bool,
    /// The link field has a menu pump. `None` when this row set has no field to drive.
    pub menu_pump: Option<bool>,
    /// The import and the export have a game task to finish on. `None` when this row set has
    /// nothing to import or export.
    pub game_task: Option<bool>,
    /// The profile-renderer table is guarded, so opening `05_010_ProfileSelect` cannot fault in the
    /// native refresh. `None` when no row in this set opens that window.
    pub profile_table_guard: Option<bool>,
}

impl StandaloneArm {
    /// True only when every part this row set needs is in place.
    ///
    /// A [`None`] is not a missing part: it is machinery the armed rows never reach, so it is
    /// neither installed nor required. Only `Some(false)` -- asked for and refused -- fails.
    pub fn is_complete(&self) -> bool {
        self.gfx_served
            && self.rows_armed
            && self.menu_pump != Some(false)
            && self.game_task != Some(false)
            && self.profile_table_guard != Some(false)
    }
}

/// Arm `rows` with no product behind them, routing presses to `actions`.
///
/// Call from a bootstrap thread, not from `DllMain`: the game-task registration waits for the
/// game's task manager to exist, and waiting inside the loader lock deadlocks the process.
///
/// `actions` must carry an entry for every row in `rows` that this crate does not drive itself.
/// The build rows are driven from inside the crate and need none; the two character rows need the
/// opener a shell supplies. A `None` beside a row that is in the set is a row that appears and does
/// nothing, which is worse than absent -- so a shell that cannot supply a flow must leave that row
/// out of `rows` rather than out of `actions`.
///
/// # Safety
///
/// Bootstrap thread, once per process, before the Quit tab has built a dialog.
pub unsafe fn arm_standalone(rows: RowSet, actions: QuitRowActions) -> StandaloneArm {
    // First, because it is the only one with a deadline: the movie is served the first time the
    // Quit tab is opened, and a swap registered after that shows a vanilla two-cell grid until the
    // panel is rebuilt.
    let gfx_served = unsafe { crate::gfx_swap::install_quit_menu_gfx_swap_hook() };
    let rows_armed = match unsafe { crate::row_cloner::arm(rows, actions) } {
        Ok(()) => true,
        Err(ArmError::AlreadyArmed) => {
            append_autoload_debug(format_args!(
                "system-quit-dup: already armed in this process; leaving the first arm's rows alone"
            ));
            false
        }
        Err(error) => {
            append_autoload_debug(format_args!(
                "system-quit-dup: could not arm the Quit rows: {error:?}"
            ));
            false
        }
    };
    // The link field and the importer are the only reasons the game task exists, so a row set
    // without a build row neither installs nor needs it.
    let build_rows = rows.load_build_from_url || rows.generate_build_link;
    let game_task = build_rows.then(crate::game_task::install_build_row_game_task);
    // Both character rows open `05_010_ProfileSelect`, and that window renders a character model
    // per slot. The native refresh that draws them walks the renderer table without a null check,
    // so a host arming either row has to own the guard or the first press is an access violation
    // rather than a row -- measured 2026-09-11, `0xc0000005` at `eldenring.exe+0x9ab874`.
    // The product installs the same body from its own private detour and must not call this.
    let character_rows = rows.load_character || rows.load_character_from_file;
    let profile_table_guard = character_rows
        .then(|| unsafe { crate::profile_table_guard::install_profile_table_guard() });
    // One detour, two reasons to want it. The link field needs a menu pump to submit its keyboard
    // job; a character row needs the same post-run moment to hide the pause menu behind the picker
    // it just opened and to put it back when the picker closes. Neither is the product's hook --
    // this is the shell's own, chained onto the same address through the union.
    crate::menu_pump::set_character_rows_armed(character_rows);
    let menu_pump = (build_rows || character_rows)
        .then(|| unsafe { crate::menu_pump::install_quit_menu_window_run_hook() });
    let arm = StandaloneArm {
        gfx_served,
        rows_armed,
        menu_pump,
        game_task,
        profile_table_guard,
    };
    // `not-required` rather than `None`: the line is read by a person looking for what went wrong,
    // and a bare `None` beside three booleans reads as a failure that printed oddly.
    let describe = |part: Option<bool>| match part {
        Some(true) => "yes",
        Some(false) => "FAILED",
        None => "not-required",
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: standalone arm complete={} gfx_served={gfx_served} rows_armed={rows_armed} menu_pump={} game_task={} profile_table_guard={} rows={rows:?}",
        arm.is_complete(),
        describe(menu_pump),
        describe(game_task),
        describe(profile_table_guard)
    ));
    arm
}

/// Emit the build rows' telemetry counters as one machine-readable line.
///
/// A standalone shell writes no `er-quickload-telemetry.json`, so until this existed the only
/// record a shell run left behind was prose -- readable by a person, not assertable by a watcher,
/// and the reason a row press could only ever be reported as "seen in the log" rather than
/// measured. Every value here is a count the DLL derived from the game's own memory: a placement
/// counted only when the root proxy accepted a transform, an open counted only when the field's
/// window ran, an opened link counted only when `ShellExecuteW` said so.
///
/// Called at each row outcome rather than at teardown, because a shell has no teardown hook and a
/// run that crashes still leaves the last outcome's line on disk.
pub fn append_build_row_oracle_line(reason: &str) {
    use er_telemetry_core::counters as c;
    let load = |counter: &'static core::sync::atomic::AtomicUsize| counter.load(Ordering::SeqCst);
    // Each name says what its counter counts, because the first live run made the cost of not
    // doing that concrete. `url_requests` was `..._REQUEST_COUNT`, which counts imports handed to
    // the importer, while `link_requests` was the generate row's press. A cancelled link field
    // therefore printed `url_requests=0 link_requests=1`, which reads as "the build-url row never
    // fired" -- and the row had fired: it opened the field, the player backed out, and no import
    // was ever requested. The press counter it should have been showing was in the same module
    // and simply absent from the line.
    append_autoload_debug(format_args!(
        "system-quit-rows: oracle at={reason} \
         url_row_presses={} url_imports_requested={} url_editor_opens={} \
         url_window_placed={} url_window_unplaced={} \
         url_accepted={} url_cancelled={} url_imported={} url_rejected={} url_failed={} \
         link_row_presses={} link_exports_requested={} link_encoded={} link_clipboard={} \
         link_opened={} link_failed={} link_url_len={}",
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_COUNT),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_REQUEST_COUNT),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_EDITOR_OPEN_COUNT),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_WINDOW_PLACED),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_WINDOW_UNPLACED),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_ACCEPTED_COUNT),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_CANCELLED_COUNT),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_IMPORTED_COUNT),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_REJECTED_COUNT),
        load(&c::SYSTEM_QUIT_LOAD_BUILD_URL_FAILED_COUNT),
        load(&c::SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_COUNT),
        load(&c::SYSTEM_QUIT_GENERATE_BUILD_LINK_REQUEST_COUNT),
        load(&c::SYSTEM_QUIT_GENERATE_BUILD_LINK_ENCODED_COUNT),
        load(&c::SYSTEM_QUIT_GENERATE_BUILD_LINK_CLIPBOARD_COUNT),
        load(&c::SYSTEM_QUIT_GENERATE_BUILD_LINK_OPENED_COUNT),
        load(&c::SYSTEM_QUIT_GENERATE_BUILD_LINK_FAILED_COUNT),
        load(&c::SYSTEM_QUIT_GENERATE_BUILD_LINK_LAST_URL_LEN),
    ));
}
