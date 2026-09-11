//! Product re-export facade: the System>Quit link field moved to
//! `er_quit_menu_core::build_url_editor`.
//!
//! Pure code reorganization, no behavior change: the moved module reaches the product debug log
//! through the `QuitMenuHost` seam, and the field it drives through the software keyboard and the
//! Scaleform proxy primitives that moved to that crate with it.

pub(crate) use er_quit_menu_core::build_url_editor::*;
