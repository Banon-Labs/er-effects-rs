//! Product re-export facade: the System>Quit **Generate Build Link** row moved to
//! `er_quit_menu_core::generate_build_link_row`.
//!
//! Pure code reorganization, no behavior change. The moved module keeps its own `ShellExecuteW` and
//! clipboard sinks and reaches the product debug log through the `QuitMenuHost` seam, which
//! `DllMain` installs before any hook or task can run it.

pub(crate) use er_quit_menu_core::generate_build_link_row::*;
