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
