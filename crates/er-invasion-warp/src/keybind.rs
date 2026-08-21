//! Naming the keys that mark and un-mark a location, so a keyboard without Insert/Delete can play.
//!
//! # Why names and not numbers
//!
//! The mark keys were hard-coded to `VK_INSERT`/`VK_DELETE`. A 60% keyboard -- the common compact
//! EN-US layout -- has neither, which locks a whole feature out for anyone using one. Asking those
//! players to write `0x2d` in a config file swaps one barrier for another: nobody knows the virtual
//! key code for the key they are looking at.
//!
//! So the config takes a NAME (`"Insert"`, `"F7"`, `"KP_Plus"`, `"]"`), and a raw `0x2d`-style code
//! is accepted too for anyone who does know. An unrecognised name is an ERROR that names the key it
//! could not parse -- silently falling back to the default would leave the player pressing a key
//! that does nothing, with no way to tell that from a broken feature.

/// A Win32 virtual-key code.
pub type VirtualKey = i32;

/// `VK_INSERT` -- the historical mark key, still the default.
pub const VK_INSERT: VirtualKey = 0x2d;
/// `VK_DELETE` -- the historical un-mark key, still the default.
pub const VK_DELETE: VirtualKey = 0x2e;

/// Every key this crate will accept by name, lowercased, with its virtual-key code.
///
/// Deliberately covers what a compact keyboard actually HAS: the function row, the alphanumerics,
/// the punctuation keys, the arrows, and the numeric keypad (present on TKL but not on 60%).
/// Aliases are included where a key has more than one obvious name (`"esc"`/`"escape"`,
/// `"]"`/`"rightbracket"`) because a config file is written by a person, not a parser.
const NAMED_KEYS: &[(&str, VirtualKey)] = &[
    ("insert", VK_INSERT),
    ("ins", VK_INSERT),
    ("delete", VK_DELETE),
    ("del", VK_DELETE),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pgup", 0x21),
    ("pagedown", 0x22),
    ("pgdn", 0x22),
    ("backspace", 0x08),
    ("tab", 0x09),
    ("enter", 0x0d),
    ("return", 0x0d),
    ("escape", 0x1b),
    ("esc", 0x1b),
    ("space", 0x20),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("printscreen", 0x2c),
    ("scrolllock", 0x91),
    ("pause", 0x13),
    ("capslock", 0x14),
    // Punctuation, by symbol and by name. These survive on every compact layout.
    ("-", 0xbd),
    ("minus", 0xbd),
    ("=", 0xbb),
    ("equals", 0xbb),
    ("[", 0xdb),
    ("leftbracket", 0xdb),
    ("]", 0xdd),
    ("rightbracket", 0xdd),
    ("\\", 0xdc),
    ("backslash", 0xdc),
    (";", 0xba),
    ("semicolon", 0xba),
    ("'", 0xde),
    ("quote", 0xde),
    (",", 0xbc),
    ("comma", 0xbc),
    (".", 0xbe),
    ("period", 0xbe),
    ("/", 0xbf),
    ("slash", 0xbf),
    ("`", 0xc0),
    ("grave", 0xc0),
    // Numeric keypad. Named with a KP_ prefix so "KP_2" is never confused with the "2" row key.
    ("kp_0", 0x60),
    ("kp_1", 0x61),
    ("kp_2", 0x62),
    ("kp_3", 0x63),
    ("kp_4", 0x64),
    ("kp_5", 0x65),
    ("kp_6", 0x66),
    ("kp_7", 0x67),
    ("kp_8", 0x68),
    ("kp_9", 0x69),
    ("kp_multiply", 0x6a),
    ("kp_plus", 0x6b),
    ("kp_minus", 0x6d),
    ("kp_period", 0x6e),
    ("kp_divide", 0x6f),
];

/// Why a key name could not be turned into a virtual-key code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyParseError {
    /// The value was empty or only whitespace.
    Empty,
    /// The name is not one this crate knows.
    Unknown(String),
    /// A `0x..`/decimal code that is outside the virtual-key range `1..=254`.
    OutOfRange(i64),
}

impl std::fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "no key given"),
            Self::Unknown(name) => write!(
                f,
                "unknown key {name:?}. Use a name like \"Insert\", \"F7\", \"KP_Plus\" or \"]\", a \
                 single letter or digit, or a raw virtual-key code such as 0x2d"
            ),
            Self::OutOfRange(value) => write!(
                f,
                "virtual-key code {value:#x} is outside the usable range 0x01..=0xFE"
            ),
        }
    }
}

/// Turn a config value into a virtual-key code.
///
/// Accepts, in order: a single letter or digit (`"K"`, `"7"`), a function key (`"F1"`..`"F24"`), a
/// name or symbol from [`NAMED_KEYS`], or a raw code (`"0x2d"`, `"45"`).
///
/// # Errors
/// Returns [`KeyParseError`] when the value is empty, unrecognised, or a code outside `1..=254`.
pub fn parse_key(value: &str) -> Result<VirtualKey, KeyParseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KeyParseError::Empty);
    }
    let lower = trimmed.to_ascii_lowercase();

    // A single letter or digit maps to its own ASCII code -- that is exactly how Win32 numbers
    // them, so no table is needed and every layout's alphanumerics work.
    if lower.len() == 1 {
        let ch = lower.as_bytes()[0];
        if ch.is_ascii_lowercase() {
            return Ok(VirtualKey::from(ch.to_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return Ok(VirtualKey::from(ch));
        }
    }

    // Function keys are contiguous from VK_F1, so they are computed rather than listed.
    if let Some(rest) = lower.strip_prefix('f')
        && let Ok(index) = rest.parse::<u8>()
        && (1..=24).contains(&index)
    {
        return Ok(0x70 + VirtualKey::from(index - 1));
    }

    if let Some((_, code)) = NAMED_KEYS.iter().find(|(name, _)| *name == lower) {
        return Ok(*code);
    }

    // A raw code, last, so a name never gets misread as a number.
    let raw = if let Some(hex) = lower.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()
    } else {
        lower.parse::<i64>().ok()
    };
    if let Some(raw) = raw {
        if (1..=0xFE).contains(&raw) {
            return Ok(raw as VirtualKey);
        }
        return Err(KeyParseError::OutOfRange(raw));
    }

    Err(KeyParseError::Unknown(trimmed.to_string()))
}

/// The name this crate would print for a virtual-key code, for echoing config back to the player.
///
/// Falls back to the raw code when the key has no name in the table -- a code we accepted through
/// the raw-number path has no name to give.
#[must_use]
pub fn key_name(code: VirtualKey) -> String {
    if (0x70..=0x87).contains(&code) {
        return format!("F{}", code - 0x70 + 1);
    }
    if let Some((name, _)) = NAMED_KEYS.iter().find(|(_, c)| *c == code) {
        let mut chars = name.chars();
        return match chars.next() {
            Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
            None => name.to_string(),
        };
    }
    if (0x30..=0x39).contains(&code) || (0x41..=0x5a).contains(&code) {
        // SAFETY-FREE: both ranges are ASCII digits and uppercase letters by construction.
        return char::from(code as u8).to_string();
    }
    format!("{code:#04x}")
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

    /// THE FAILURE THAT MATTERS. A typo must say so, not silently keep the old key -- otherwise the
    /// player presses their chosen key, nothing happens, and nothing tells them why.
    #[test]
    fn an_unknown_name_is_an_error_that_names_the_offending_key() {
        let error = parse_key("Winkey").expect_err("not a key this crate knows");
        assert_eq!(error, KeyParseError::Unknown("Winkey".to_string()));
        assert!(error.to_string().contains("Winkey"), "{error}");
        // And it suggests what WOULD work.
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

    /// Every name in the table round-trips through the parser -- a typo'd table entry would
    /// otherwise be a key nobody can select.
    #[test]
    fn every_named_key_parses_to_its_own_code() {
        for (name, code) in NAMED_KEYS {
            assert_eq!(parse_key(name), Ok(*code), "{name}");
        }
    }

    /// Names are echoed back for the log line that tells the player which keys are live.
    #[test]
    fn a_code_renders_a_name_the_player_would_recognise() {
        assert_eq!(key_name(VK_INSERT), "Insert");
        assert_eq!(key_name(0x76), "F7");
        assert_eq!(key_name(0x4b), "K");
        assert_eq!(key_name(0x6b), "Kp_plus");
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
