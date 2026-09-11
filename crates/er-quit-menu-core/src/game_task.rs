//! The recurring game task the two build rows need, for a host that has no task of its own.
//!
//! A row press only latches a request. The import mutates the inventory, the `CSGaitemImp`
//! singleton, `PlayerGameData` and the equipment slots, and it begins with a blocking network get;
//! neither belongs on whichever thread dispatched the menu activation. So the work runs on a
//! `FrameBegin` task instead, which is the context every mutation inside the runtime requires.
//!
//! The product already registers such a task and calls both ticks from it. A standalone shell does
//! not, so this registers the same pair the same way -- the shape `er-build-import`'s own shell
//! established.

use crate::host::append_autoload_debug;

/// Register the `FrameBegin` task that drives the import and the export.
///
/// Blocks on the game's task manager appearing, so it must be called from a bootstrap thread rather
/// than from `DllMain` itself.
///
/// Returns false when the task manager never resolved, which means neither row will ever apply
/// anything -- a press would latch a request that nothing consumes.
pub fn install_build_row_game_task() -> bool {
    use eldenring::cs::{CSTaskGroupIndex, CSTaskImp};
    use eldenring::fd4::FD4TaskData;
    use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

    // Bounded: see `er_game_base::wait` -- the unbounded form of this loop starved the wineserver
    // and hung a boot.
    let Some(task) = er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
    else {
        append_autoload_debug(format_args!(
            "system-quit-build-url: CSTaskImp never resolved; the build rows would latch requests nothing consumes"
        ));
        return false;
    };
    let handle = task.run_recurring(
        move |_data: &FD4TaskData| {
            // Safety: this closure runs on the game task thread, which is the context both ticks
            // require; each step inside them is individually precondition-checked and returns
            // immediately unless a row press latched a request.
            unsafe { crate::build_url_row::system_quit_build_import_tick() };
            unsafe { crate::generate_build_link_row::system_quit_build_export_tick() };
        },
        CSTaskGroupIndex::FrameBegin,
    );
    // The handle cancels the task on drop, and the task must outlive the bootstrap thread.
    std::mem::forget(handle);
    append_autoload_debug(format_args!(
        "system-quit-build-url: registered the FrameBegin task that applies an import and finishes an export"
    ));
    true
}
