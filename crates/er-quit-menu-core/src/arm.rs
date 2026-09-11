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
    /// The link field has a menu pump.
    pub menu_pump: bool,
    /// The import and the export have a game task to finish on.
    pub game_task: bool,
}

impl StandaloneArm {
    /// True only when every part a row needs to work is in place.
    pub fn is_complete(&self) -> bool {
        self.gfx_served && self.rows_armed && self.menu_pump && self.game_task
    }
}

/// Arm `rows` with no product behind them.
///
/// Call from a bootstrap thread, not from `DllMain`: the game-task registration waits for the
/// game's task manager to exist, and waiting inside the loader lock deadlocks the process.
///
/// The row actions are left at their defaults on purpose. A shell has no character-switch flow, no
/// save browser and no Save Game commit, and the rows that would reach them are not in `rows` -- so
/// the table stays empty rather than carrying a pointer to something that does not exist.
///
/// # Safety
///
/// Bootstrap thread, once per process, before the Quit tab has built a dialog.
pub unsafe fn arm_standalone(rows: RowSet) -> StandaloneArm {
    // First, because it is the only one with a deadline: the movie is served the first time the
    // Quit tab is opened, and a swap registered after that shows a vanilla two-cell grid until the
    // panel is rebuilt.
    let gfx_served = unsafe { crate::gfx_swap::install_quit_menu_gfx_swap_hook() };
    let rows_armed = match unsafe { crate::row_cloner::arm(rows, QuitRowActions::default()) } {
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
    let menu_pump = unsafe { crate::menu_pump::install_quit_menu_window_run_hook() };
    let game_task = crate::game_task::install_build_row_game_task();
    let arm = StandaloneArm {
        gfx_served,
        rows_armed,
        menu_pump,
        game_task,
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: standalone arm complete={} gfx_served={gfx_served} rows_armed={rows_armed} menu_pump={menu_pump} game_task={game_task} rows={rows:?}",
        arm.is_complete()
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
