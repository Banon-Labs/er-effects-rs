//! Cutting a build's display name down to something Elden Ring will accept as a character name.
//!
//! The game has no rename UI, so the only way to give an imported character the build's name is to
//! call `CS::PlayerGameData::CopyChrName` -- and that native validates nothing it needs to. This
//! module is the validation, kept here rather than beside the call because it is pure and this is
//! the half `cargo test` can reach. The call itself is
//! `er_build_import_runtime::chr_name::adopt_build_name`, which pins [`CHR_NAME_MAX_UNITS`] below
//! against the real `PlayerGameData` layout so the two cannot drift.

/// UTF-16 code units a character name may actually contain.
///
/// `PlayerGameData::characterName` is a `wchar_t[17]` (`+0x9c`..`+0xbe`), and the last unit is the
/// terminator -- but the binding number is the native writer's own gate, which is literally
/// `if (wcslen(name) < 0x11)`. A 17-unit name would fit the array and still be refused, silently,
/// leaving the old name in place and the caller with nothing to report.
pub const CHR_NAME_MAX_UNITS: usize = 16;

/// A build name cut down to something `CopyChrName` will accept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClampedName {
    /// The clamped text, for logging and for the caller's read-back comparison.
    pub text: String,
    /// The same text as a NUL-terminated UTF-16 buffer -- the argument the native takes.
    pub buffer: Vec<u16>,
    /// Whether anything was dropped for length.
    pub truncated: bool,
}

/// Cut a build's display name down to a character name, or `None` when nothing usable is left.
///
/// # The UTF-16 handling, which is the whole of this function
///
/// Three separate ways a name can be wrong here, and the native catches none of them:
///
/// 1. **Length is counted in UTF-16 code units, not chars and not bytes.** `CopyChrName` compares
///    `wcslen(name) < 0x11`. An emoji or any other astral character is two units, so a 16-character
///    name can be a 32-unit string. Counting `chars()` would build a buffer the native silently
///    refuses; counting `len()` (bytes) would cut a name far shorter than it needs to be. This
///    accumulates `char::len_utf16` and stops before the budget is exceeded, so a surrogate pair is
///    never split -- a lone surrogate is not valid UTF-16, and is what a byte- or char-based cut
///    produces.
/// 2. **An interior NUL truncates the name silently.** A Rust `String` may contain `U+0000`; the
///    native measures with `wcslen`, so everything after the first NUL would vanish without a word.
///    Control characters are dropped for the same class of reason -- they render as boxes, and this
///    name goes out to other players.
/// 3. **The buffer must be NUL-terminated.** `CopyChrName` runs `wcslen` on the argument before it
///    validates anything, so handing it an unterminated slice is an out-of-bounds read of the
///    caller's memory, not a rejected name. The terminator is pushed here, once, rather than left
///    to a caller to remember.
///
/// ```
/// use er_build_import_core::chr_name::clamp_to_field;
/// let clamped = clamp_to_field("Steelovsky Malenia Killer").expect("a usable name");
/// assert_eq!(clamped.text, "Steelovsky Malen");
/// assert!(clamped.truncated);
/// assert_eq!(clamped.buffer.last(), Some(&0));
/// ```
pub fn clamp_to_field(build_name: &str) -> Option<ClampedName> {
    let cleaned: String = build_name.chars().filter(|ch| !ch.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut units = 0usize;
    for ch in cleaned.chars() {
        let width = ch.len_utf16();
        if units + width > CHR_NAME_MAX_UNITS {
            break;
        }
        units += width;
        text.push(ch);
    }
    let truncated = text.len() < cleaned.len();
    // A cut can land mid-word and leave a trailing space: `"Steelovsky Malenia"` becoming
    // `"Steelovsky Malen"` is a name, `"Steelovsky "` is a typo.
    let text = text.trim_end().to_owned();
    if text.is_empty() {
        return None;
    }

    let mut buffer: Vec<u16> = text.encode_utf16().collect();
    buffer.push(0);
    Some(ClampedName {
        text,
        buffer,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::{CHR_NAME_MAX_UNITS, clamp_to_field};

    /// The buffer handed to the native must always be terminated -- it is `wcslen`'d before it is
    /// validated, so an unterminated one is an out-of-bounds read rather than a rejected name.
    #[test]
    fn the_buffer_is_always_nul_terminated() {
        for name in ["Bean Smith", "x", &"a".repeat(200)] {
            let clamped = clamp_to_field(name).expect("a usable name");
            assert_eq!(
                clamped.buffer.last(),
                Some(&0),
                "{name:?} lost its terminator"
            );
        }
    }

    /// The native's gate is `wcslen(name) < 0x11`, so the text must never reach 17 units.
    #[test]
    fn nothing_ever_exceeds_the_field() {
        for name in [
            "Bean Smith",
            &"a".repeat(200),
            "\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}",
            "\u{395}\u{3bb}\u{3bb}\u{3b7}\u{3bd}\u{3b9}\u{3ba}\u{3ac} \u{3bf}\u{3bd}\u{3cc}\u{3bc}\u{3b1}\u{3c4}\u{3b1} \u{3b5}\u{3b4}\u{3ce}",
        ] {
            let clamped = clamp_to_field(name).expect("a usable name");
            let units = clamped.text.encode_utf16().count();
            assert!(
                units <= CHR_NAME_MAX_UNITS,
                "{name:?} clamped to {units} units"
            );
            assert_eq!(clamped.buffer.len(), units + 1);
        }
    }

    /// Astral characters are two units each. Ten of them are twenty units, so eight survive and the
    /// cut lands between characters -- never between a high and low surrogate, which is what a
    /// char-count or byte-count cut produces and which is not valid UTF-16 at all.
    #[test]
    fn a_surrogate_pair_is_never_split() {
        let clamped = clamp_to_field(
            "\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}\u{1f5ff}",
        )
        .expect("a usable name");
        assert_eq!(clamped.text.chars().count(), 8);
        assert_eq!(clamped.text.encode_utf16().count(), CHR_NAME_MAX_UNITS);
        assert!(clamped.truncated);
        assert!(
            String::from_utf16(&clamped.buffer[..clamped.buffer.len() - 1]).is_ok(),
            "the clamped buffer is not valid UTF-16, so a pair was split"
        );
    }

    /// An interior NUL would make `wcslen` stop early and silently drop the rest of the name.
    #[test]
    fn control_characters_including_nul_are_dropped() {
        let clamped = clamp_to_field("Bean\0 Smith\n").expect("a usable name");
        assert_eq!(clamped.text, "Bean Smith");
        assert!(!clamped.buffer[..clamped.buffer.len() - 1].contains(&0));
    }

    /// A name that is only whitespace or only control characters is not a name. Blanking the
    /// character's name instead would trip the game's own
    /// `ReplaceCharacterNameWithDefaultIfEmpty`, which renames it to a stock string.
    #[test]
    fn an_unusable_name_is_refused_rather_than_blanked() {
        assert!(clamp_to_field("").is_none());
        assert!(clamp_to_field("   ").is_none());
        assert!(clamp_to_field("\0\0\0").is_none());
    }

    /// A cut that lands on a space must not leave one dangling.
    #[test]
    fn a_truncated_name_does_not_end_in_a_space() {
        let clamped = clamp_to_field("Steelovsky Malenia Killer").expect("a usable name");
        assert_eq!(clamped.text, "Steelovsky Malen");
        assert!(clamped.truncated);
        // The 16th unit of this one is the space, so the trim is what makes the difference between
        // `"Steelovsky Male"` and `"Steelovsky Male "`.
        let on_a_space = clamp_to_field("Steelovsky Male nia").expect("a usable name");
        assert_eq!(on_a_space.text, "Steelovsky Male");
        assert!(on_a_space.truncated);
    }

    /// The names decoded out of real build links must pass through unchanged.
    #[test]
    fn the_real_build_names_survive_unchanged() {
        for name in ["Steelovsky M", "Steelovsky S", "Bean Smith"] {
            let clamped = clamp_to_field(name).expect("a usable name");
            assert_eq!(clamped.text, name);
            assert!(!clamped.truncated);
        }
    }
}
