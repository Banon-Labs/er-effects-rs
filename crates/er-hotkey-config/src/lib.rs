//! Player-nameable hotkeys that take effect without restarting the game.
//!
//! # The incident this crate exists for
//!
//! `er-invasion-warp` polled a hard-coded `VK_F7` every frame. Another DLL in the same me3 profile
//! had picked the same default, so a keypress meant for one of them warped the player mid-session,
//! and nothing in either config file could separate them. The same shape was everywhere: a `VK_*`
//! constant, no config key, no way to change it, and -- where a config key did exist -- no way to
//! change it without quitting the game.
//!
//! # What a DLL gets from here
//!
//! * [`keys`] -- one table of key names (`"F7"`, `"]"`, `"KP_Plus"`, `"Insert"`) carrying both
//!   numbering schemes a key reaches this process by: Win32 virtual keys and DirectInput
//!   scancodes.
//! * [`reload`] -- [`reload::HotFile`], which notices a config file changed by comparing its text
//!   (not its mtime, which has one-second resolution on the filesystems a Wine prefix tends to sit
//!   on) and throttles itself to roughly one read per second.
//! * [`binding`] -- [`binding::Binding`], which turns "the file changed" into one of exactly three
//!   outcomes: the key moved (reset your edge detector), the key did not move (do not reset it), or
//!   the value was junk and the last working key is still in force.
//! * [`pad`] -- the controller half of the same vocabulary: [`pad::PadChord`], a BITMASK over
//!   `XINPUT_GAMEPAD.wButtons` (a pad has no modifiers and no scancodes, so it is a different
//!   type rather than a field bolted onto [`keys::Chord`]), and [`pad::PadEdge`], whose rebind
//!   rule is the pad's version of the phantom-press fix in [`binding`].
//! * [`live`] -- [`live::AtomicChord`], a binding the detour that actually reads the keyboard can
//!   load without touching a lock the reload path also wants.
//! * [`persist`] -- the other direction: writing one setting back into a file the player also
//!   owns, by rewriting a single line in place and renaming it over the original, so a state a
//!   hotkey changes in-game survives a relaunch without the DLL eating their comments.
//!
//! # What it deliberately does not do
//!
//! It does not read the keyboard, own a config file's schema, or know what a key means. Each DLL
//! keeps its own file, its own key names, and its own hook -- this crate is the vocabulary and the
//! reload decision, which are the two parts that were being reinvented differently each time.

pub mod binding;
pub mod keys;
pub mod live;
pub mod pad;
pub mod persist;
pub mod reload;

pub use binding::{Binding, BindingUpdate};
pub use keys::{
    Chord, KeyParseError, Scancode, VirtualKey, chord_down, chord_name, parse_chord,
    parse_scancode, parse_scancode_chord, parse_virtual_key, scancode_name, vk_name,
};
pub use live::AtomicChord;
pub use pad::{PadChord, PadEdge, PadParseError, pad_chord_name, parse_pad_chord};
pub use persist::{set_scalar, write_atomic};
pub use reload::{FileChange, HotFile};
