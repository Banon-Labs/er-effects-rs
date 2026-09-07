//! What a DLL does with a key name once the file it came from has changed.
//!
//! Three rules, and each exists because breaking it produces a failure the player cannot tell from
//! a broken feature:
//!
//! 1. **A malformed value keeps the previous working key.** Falling back to the built-in default
//!    silently moves the binding somewhere the player did not ask for; disabling the feature
//!    removes it entirely. Both leave a typo looking like a crash. [`BindingUpdate::Rejected`]
//!    carries the value that failed and the key still in force, so the log line can say both.
//! 2. **A value that parses to the same key is not a change.** A reload resets the edge detector,
//!    and a reset while the key is held fires it again -- a press the player never made. Reformatting
//!    a config file, or writing `"f7"` where it said `"F7"`, must not do that.
//! 3. **A real change says so.** [`BindingUpdate::Changed`] carries both keys so the caller logs
//!    `F7 -> F8` rather than "the config reloaded", which is a fact nobody needs.
//!
//! The caller owns the edge state, so it owns the reset: `Changed` is the signal to clear it. See
//! [`BindingUpdate::changed`].

use crate::keys::KeyParseError;

/// What applying a config value did to a binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingUpdate<C> {
    /// The value parsed to the key already in force. Nothing to do, and specifically no edge reset.
    Unchanged,
    /// The binding moved. The caller must reset its key edge state, or a key held at this moment
    /// reads as a fresh press.
    Changed {
        /// The key that was in force.
        from: C,
        /// The key now in force.
        to: C,
    },
    /// The value did not parse. The previous key is still in force -- this is not a disabled
    /// feature, and the log line must say so.
    Rejected {
        /// The text that could not be read, verbatim, so the player can find it in their file.
        value: String,
        /// Why it could not be read.
        error: KeyParseError,
        /// The key that remains in force.
        kept: C,
    },
}

impl<C: Copy> BindingUpdate<C> {
    /// Did the binding actually move? This is the edge-reset signal.
    #[must_use]
    pub const fn changed(&self) -> bool {
        matches!(self, Self::Changed { .. })
    }

    /// The key in force after this update, whatever happened.
    #[must_use]
    pub const fn in_force(&self, before: C) -> C {
        match self {
            Self::Unchanged => before,
            Self::Changed { to, .. } => *to,
            Self::Rejected { kept, .. } => *kept,
        }
    }
}

/// One key binding and the rules above.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding<C> {
    code: C,
}

impl<C: Copy + PartialEq> Binding<C> {
    /// A binding starting at its built-in default.
    pub const fn new(code: C) -> Self {
        Self { code }
    }

    /// The key in force.
    pub const fn code(&self) -> C {
        self.code
    }

    /// Force a key without going through a config value. For a caller that resolved one itself.
    pub const fn set(&mut self, code: C) {
        self.code = code;
    }

    /// Apply a config value, with `parse` deciding what the text means.
    ///
    /// `parse` is a parameter rather than a fixed function because the same config vocabulary
    /// resolves to a virtual key for a DLL that polls `GetAsyncKeyState` and to a scancode for one
    /// that reads the game's own DirectInput buffer -- see [`crate::keys`].
    pub fn apply(
        &mut self,
        value: &str,
        parse: impl FnOnce(&str) -> Result<C, KeyParseError>,
    ) -> BindingUpdate<C> {
        match parse(value) {
            Ok(code) if code == self.code => BindingUpdate::Unchanged,
            Ok(code) => {
                let from = self.code;
                self.code = code;
                BindingUpdate::Changed { from, to: code }
            }
            Err(error) => BindingUpdate::Rejected {
                value: value.trim().to_string(),
                error,
                kept: self.code,
            },
        }
    }

    /// Apply a config value that may be absent. An absent key is not an error and not a change:
    /// the file simply does not mention this binding, so whatever is in force stays.
    pub fn apply_optional(
        &mut self,
        value: Option<&str>,
        parse: impl FnOnce(&str) -> Result<C, KeyParseError>,
    ) -> BindingUpdate<C> {
        match value {
            Some(value) => self.apply(value, parse),
            None => BindingUpdate::Unchanged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{parse_scancode, parse_virtual_key};

    /// A changed key is picked up, and reports both ends so the log can name them.
    #[test]
    fn a_changed_key_is_picked_up_and_names_both_ends() {
        let mut binding = Binding::new(0x76u32);
        let update = binding.apply("F8", parse_virtual_key);
        assert_eq!(
            update,
            BindingUpdate::Changed {
                from: 0x76,
                to: 0x77
            }
        );
        assert!(update.changed(), "the caller must reset its edge state");
        assert_eq!(binding.code(), 0x77);
    }

    /// A value that means the same key is not a change. Reporting one resets the edge detector,
    /// and a key held at that instant then fires without being pressed.
    #[test]
    fn a_value_that_means_the_same_key_is_not_a_change() {
        let mut binding = Binding::new(0x76u32);
        for spelling in ["F7", "f7", "  F7  ", "0x76", "118"] {
            let update = binding.apply(spelling, parse_virtual_key);
            assert_eq!(update, BindingUpdate::Unchanged, "{spelling}");
            assert!(!update.changed(), "{spelling}");
        }
        assert_eq!(binding.code(), 0x76);
    }

    /// The rule that matters. A typo keeps the key that was working -- it does not fall back to
    /// the built-in default, and it does not turn the feature off.
    #[test]
    fn a_malformed_value_falls_back_to_the_previous_value_not_to_nothing() {
        let mut binding = Binding::new(0x76u32);
        binding.apply("F8", parse_virtual_key);
        let update = binding.apply("Winkey", parse_virtual_key);
        assert_eq!(
            update,
            BindingUpdate::Rejected {
                value: "Winkey".to_owned(),
                error: crate::keys::KeyParseError::Unknown("Winkey".to_owned()),
                // F8, the last value that worked -- not F7, the built-in default.
                kept: 0x77,
            }
        );
        assert!(!update.changed(), "a rejection must not reset edge state");
        assert_eq!(binding.code(), 0x77);
        assert_eq!(update.in_force(0x76), 0x77);
    }

    /// An empty value is a rejection too, not "no key". A blank line in a config file is a mistake
    /// with a fix, and silently unbinding gives the player nothing to fix.
    #[test]
    fn an_empty_value_keeps_the_previous_key() {
        let mut binding = Binding::new(0x76u32);
        let update = binding.apply("   ", parse_virtual_key);
        assert!(matches!(update, BindingUpdate::Rejected { .. }));
        assert_eq!(binding.code(), 0x76);
    }

    /// A key the file does not mention at all is not a rejection -- there is nothing wrong with a
    /// config that leaves a setting at its default.
    #[test]
    fn an_absent_key_is_neither_a_change_nor_a_rejection() {
        let mut binding = Binding::new(0x76u32);
        assert_eq!(
            binding.apply_optional(None, parse_virtual_key),
            BindingUpdate::Unchanged
        );
        assert_eq!(binding.code(), 0x76);
    }

    /// The same machinery serves a scancode-reading DLL, which is the reason `parse` is a
    /// parameter: one config vocabulary, two numbering schemes.
    #[test]
    fn the_same_binding_works_in_scancode_space() {
        let mut binding = Binding::new(0x2eu8);
        assert_eq!(
            binding.apply("f9", parse_scancode),
            BindingUpdate::Changed {
                from: 0x2e,
                to: 0x43
            }
        );
        let update = binding.apply("F16", parse_scancode);
        assert!(
            matches!(update, BindingUpdate::Rejected { .. }),
            "a key with no scancode can never fire, so it must not replace one that can"
        );
        assert_eq!(binding.code(), 0x43);
    }
}
