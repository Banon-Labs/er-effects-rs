//! Naming the keys this crate's features are bound to, so a keyboard without Insert/Delete/F7 can
//! play and two mods that picked the same default can be told apart.
//!
//! # Why names and not numbers
//!
//! The mark keys were hard-coded to `VK_INSERT`/`VK_DELETE` and the warp keys to `VK_F7`/`F8`/`F9`.
//! A 60% keyboard -- the common compact en-us layout -- has neither the editing cluster nor the
//! function row, which locks whole features out for anyone using one. Worse, F7 was also another
//! mod's default in the same me3 profile, and a live session warped on a keypress meant for the
//! other one, with no config key on either side to move. Asking players to write `0x2d` in a config
//! file swaps one barrier for another: nobody knows the virtual-key code for the key they are
//! looking at.
//!
//! So the config takes a name (`"Insert"`, `"F7"`, `"KP_Plus"`, `"]"`), and a raw `0x2d`-style code
//! is accepted too for anyone who does know.
//!
//! # Where the table lives now
//!
//! In `er-hotkey-config`, shared with every other DLL in this workspace that binds a key, so one
//! config vocabulary covers all of them. This module is the `i32` face of it: `GetAsyncKeyState`
//! takes a signed virtual key, and the config type has always stored one.
//!
//! An unrecognised name is an error that names the key it could not parse -- silently falling back
//! to the default would leave the player pressing a key that does nothing, with no way to tell that
//! from a broken feature. The caller then keeps whatever key was already working; see
//! `local_invasion_config`.

pub use er_hotkey_config::keys::KeyParseError;

/// A Win32 virtual-key code, signed because that is what `GetAsyncKeyState` takes.
pub type VirtualKey = i32;

/// `VK_INSERT` -- the historical mark key, still the default.
pub const VK_INSERT: VirtualKey = 0x2d;
/// `VK_DELETE` -- the historical un-mark key, still the default.
pub const VK_DELETE: VirtualKey = 0x2e;

/// `VK_F3` -- the default switch for the filter itself, added 2026-09-09.
///
/// It has no historical value to preserve, so it is chosen rather than inherited: F3 is
/// clear of the three warp keys, of the mark pair, and of the game's own bindings.
pub const VK_F3: VirtualKey = 0x72;

/// `VK_F7` -- the historical "warp to the nearest invasion point" key, still the default.
pub const VK_F7: VirtualKey = 0x76;
/// `VK_F8` -- the historical "next point in the catalog's order" key, still the default.
pub const VK_F8: VirtualKey = 0x77;
/// `VK_F9` -- the historical "first point in another area" key, still the default.
pub const VK_F9: VirtualKey = 0x78;

/// Turn a config value into a virtual-key code.
///
/// Accepts a single letter or digit (`"K"`, `"7"`), a function key (`"F1"`..`"F24"`), a name or
/// symbol (`"Insert"`, `"KP_Plus"`, `"]"`), or a raw code (`"0x2d"`, `"45"`).
///
/// # Errors
/// Returns [`KeyParseError`] when the value is empty, unrecognised, or a code outside `1..=254`.
pub fn parse_key(value: &str) -> Result<VirtualKey, KeyParseError> {
    // The shared parser already refuses anything outside `1..=0xFE`, so the conversion cannot
    // fail; `try_from` rather than `as` so a future widening of that range is a compile error
    // here instead of a key silently going negative.
    er_hotkey_config::keys::parse_virtual_key(value).map(|vk| VirtualKey::try_from(vk).unwrap_or(0))
}

/// The name this crate would print for a virtual-key code, for echoing config back to the player.
///
/// Falls back to the raw code when the key has no name in the table -- a code we accepted through
/// the raw-number path has no name to give.
#[must_use]
pub fn key_name(code: VirtualKey) -> String {
    u32::try_from(code).map_or_else(|_| format!("{code:#04x}"), er_hotkey_config::keys::vk_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: a keyboard without Insert/Delete can name keys it actually has.
    #[test]
    fn a_compact_keyboard_can_name_keys_it_has() {
        assert_eq!(parse_key("F7"), Ok(0x76));
        assert_eq!(parse_key("]"), Ok(0xdd));
        assert_eq!(parse_key("RightBracket"), Ok(0xdd));
        assert_eq!(parse_key("K"), Ok(0x4b));
        assert_eq!(parse_key("7"), Ok(0x37));
    }

    /// The defaults still parse, so an existing config keeps working untouched.
    #[test]
    fn the_historical_defaults_still_parse() {
        assert_eq!(parse_key("Insert"), Ok(VK_INSERT));
        assert_eq!(parse_key("Delete"), Ok(VK_DELETE));
        assert_eq!(parse_key("ins"), Ok(VK_INSERT));
        assert_eq!(parse_key("DEL"), Ok(VK_DELETE));
        assert_eq!(parse_key("F7"), Ok(VK_F7));
        assert_eq!(parse_key("F8"), Ok(VK_F8));
        assert_eq!(parse_key("F9"), Ok(VK_F9));
    }

    /// Case and surrounding whitespace are a person writing a file, not an error.
    #[test]
    fn names_are_case_insensitive_and_trimmed() {
        assert_eq!(parse_key("  kP_pLuS "), Ok(0x6b));
        assert_eq!(parse_key("ESCAPE"), Ok(0x1b));
    }

    /// A raw code is accepted for anyone who already knows it.
    #[test]
    fn a_raw_virtual_key_code_is_accepted_in_hex_or_decimal() {
        assert_eq!(parse_key("0x2d"), Ok(VK_INSERT));
        assert_eq!(parse_key("45"), Ok(VK_INSERT));
    }

    /// A name must never be misread as a number: "F7" is a key, not hex 0xF7.
    #[test]
    fn a_named_key_is_not_reinterpreted_as_a_number() {
        assert_eq!(parse_key("F7"), Ok(0x76));
        assert_ne!(parse_key("F7"), Ok(0xf7));
    }

    /// The failure that matters. A typo must say so, not silently keep the old key -- otherwise the
    /// player presses their chosen key, nothing happens, and nothing tells them why.
    #[test]
    fn an_unknown_name_is_an_error_that_names_the_offending_key() {
        let error = parse_key("Winkey").expect_err("not a key this crate knows");
        assert_eq!(error, KeyParseError::Unknown("Winkey".to_string()));
        assert!(error.to_string().contains("Winkey"), "{error}");
        // And it suggests what would work.
        assert!(error.to_string().contains("Insert"), "{error}");
    }

    #[test]
    fn an_empty_value_is_an_error_rather_than_a_default() {
        assert_eq!(parse_key(""), Err(KeyParseError::Empty));
        assert_eq!(parse_key("   "), Err(KeyParseError::Empty));
    }

    /// A code outside the virtual-key range can never fire, so accepting it would be a silent dud.
    #[test]
    fn an_out_of_range_code_is_refused() {
        assert_eq!(parse_key("0x0"), Err(KeyParseError::OutOfRange(0)));
        assert_eq!(parse_key("0x1ff"), Err(KeyParseError::OutOfRange(0x1ff)));
    }

    /// Every key the config file's own documentation lists must parse. The list is copied into the
    /// generated TOML, so a name in the comments that the parser rejects is a lie the player reads.
    #[test]
    fn every_key_the_config_comments_advertise_parses() {
        for name in [
            "Insert",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Backspace",
            "Tab",
            "Enter",
            "Escape",
            "Space",
            "Left",
            "Up",
            "Right",
            "Down",
            "PrintScreen",
            "ScrollLock",
            "Pause",
            "CapsLock",
            "Minus",
            "Equals",
            "LeftBracket",
            "RightBracket",
            "Backslash",
            "Semicolon",
            "Quote",
            "Comma",
            "Period",
            "Slash",
            "Grave",
            "KP_0",
            "KP_9",
            "KP_Plus",
            "KP_Minus",
            "KP_Multiply",
            "KP_Divide",
            "KP_Period",
            "F1",
            "F24",
        ] {
            assert!(parse_key(name).is_ok(), "{name}");
        }
    }

    /// Names are echoed back for the log line that tells the player which keys are live, and for
    /// the config file the mark keys rewrite -- so every name printed must parse back to the same
    /// code, or a mark press would rewrite the file with a key the parser then rejects.
    #[test]
    fn a_code_renders_a_name_that_parses_back_to_it() {
        for code in [VK_INSERT, VK_DELETE, VK_F7, VK_F8, VK_F9, 0x4b, 0x6b, 0xdd] {
            assert_eq!(parse_key(&key_name(code)), Ok(code), "{code:#04x}");
        }
        assert_eq!(key_name(VK_INSERT), "Insert");
        assert_eq!(key_name(0x76), "F7");
        assert_eq!(key_name(0x4b), "K");
        assert_eq!(key_name(0x6b), "KP_Plus");
    }

    /// Function keys are computed, so the whole range must be right at both ends.
    #[test]
    fn the_function_key_range_is_correct_at_both_ends() {
        assert_eq!(parse_key("F1"), Ok(0x70));
        assert_eq!(parse_key("F24"), Ok(0x87));
        assert!(matches!(parse_key("F25"), Err(KeyParseError::Unknown(_))));
        assert!(matches!(parse_key("F0"), Err(KeyParseError::Unknown(_))));
    }
}
