//! Product re-export facade: the native `CS::SoftwareKeyboard` job and the two text fields it
//! opens moved to `er_quit_menu_core::software_keyboard`.
//!
//! Pure code reorganization, no behavior change. The moved module reaches the picker's browse
//! surface and the path editor's caret latch through the `QuitMenuHost` seam, which `DllMain`
//! installs before any hook or menu pump can run it, and its generated prologue table moved with
//! it into that crate's own `build.rs`.

pub(crate) use er_quit_menu_core::software_keyboard::*;
