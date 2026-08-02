use super::*;

// Save-destination identity moved to `er-quit-menu` in S7.
//
// This shim preserves the root DLL flat startup-hooks namespace until S8 moves the hooked surfaces.

pub(crate) use er_quit_menu::save_dest_identity::*;
