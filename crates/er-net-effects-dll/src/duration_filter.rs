//! Filtering the selectable catalog by SpEffect duration.
//!
//! WHY DURATION IS WORTH FILTERING ON. Applying an effect with network sync broadcasts it
//! (`SendSpEffectIdSync`), but removal has no network counterpart anywhere in the game -- so an
//! effect you put on other players ends for them only when its own duration expires there. An
//! effect whose `SpEffectParam.effectEndurance` is `-1` never expires, and is therefore permanent
//! for everyone else until they die or reload. In the shipped `visuals-only` catalog that is 516
//! of 842 entries, so "am I about to pick something I can never take back" is a question worth
//! being able to answer by not being offered those entries at all.
//!
//! `-1` IS THE ONLY NEGATIVE the field ever holds: across all 11325 rows of the master catalog
//! the only negative `effectEndurance` value is exactly `-1`, 3636 times. So permanence is that
//! sentinel and not a range test.

// Windows-only in practice; portable so the rules below are asserted by `cargo test` on the host.
#![cfg_attr(not(windows), allow(dead_code))]

/// `SpEffectParam.effectEndurance` for an effect that never expires on its own.
pub(crate) const PERMANENT_ENDURANCE: f32 = -1.0;

/// What the master catalog's omission of `effectEndurance` means.
///
/// The generator drops any field equal to its PARAMDEF default, and `SpEffect.xml` declares
/// `f32 effectEndurance` with no `= default`, so the generator's fallback of `0` applies: an
/// entry with no `effectEndurance` has a duration of zero (a one-shot), NOT a permanent one.
/// Reading absence as "unknown" would have hidden 3089 instant effects from the `only` mode and
/// leaked them into `exclude`.
pub(crate) const PARAMDEF_DEFAULT_ENDURANCE: f32 = 0.0;

/// The `permanent_effects` setting in `er-net-effects.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PermanentEffects {
    /// Offer everything. The default, and what the selector did before this setting existed.
    #[default]
    Include,
    /// Hide effects that never expire -- the ones that cannot be taken back off other players.
    Exclude,
    /// Offer ONLY the never-expiring effects.
    Only,
}

impl PermanentEffects {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "include" | "all" | "any" => Some(Self::Include),
            "exclude" | "none" | "temporary" | "temporary_only" => Some(Self::Exclude),
            "only" | "permanent" | "permanent_only" => Some(Self::Only),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
            Self::Only => "only",
        }
    }

    /// Is this setting anything other than the default pass-through?
    pub(crate) fn filters(self) -> bool {
        self != Self::Include
    }

    /// May an effect with this duration be offered?
    ///
    /// `None` is a duration we could not establish at all -- no master catalog on disk, so no
    /// `effectEndurance` for anything. Both filtering modes then refuse to CLAIM anything about
    /// an entry: `exclude` keeps it (it cannot be shown to be permanent) and `only` drops it (it
    /// cannot be shown to be permanent either). Guessing in either direction would quietly hand
    /// the player exactly the entries the setting exists to keep away from them.
    pub(crate) fn allows(self, duration: Option<f32>) -> bool {
        match self {
            Self::Include => true,
            Self::Exclude => !matches!(duration, Some(duration) if is_permanent(duration)),
            Self::Only => matches!(duration, Some(duration) if is_permanent(duration)),
        }
    }
}

/// Does this effect never expire on its own?
pub(crate) fn is_permanent(duration: f32) -> bool {
    duration == PERMANENT_ENDURANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_defaults_to_offering_everything() {
        assert_eq!(PermanentEffects::default(), PermanentEffects::Include);
        assert!(!PermanentEffects::default().filters());
        for duration in [Some(-1.0), Some(0.0), Some(30.0), None] {
            assert!(
                PermanentEffects::default().allows(duration),
                "the default must offer {duration:?}, exactly as it did before the setting existed"
            );
        }
    }

    #[test]
    fn exclude_drops_only_the_never_expiring_effects() {
        let mode = PermanentEffects::Exclude;
        assert!(!mode.allows(Some(PERMANENT_ENDURANCE)));
        assert!(
            mode.allows(Some(PARAMDEF_DEFAULT_ENDURANCE)),
            "a one-shot is not permanent"
        );
        assert!(mode.allows(Some(0.05)));
        assert!(mode.allows(Some(180.0)));
    }

    #[test]
    fn only_keeps_the_never_expiring_effects() {
        let mode = PermanentEffects::Only;
        assert!(mode.allows(Some(PERMANENT_ENDURANCE)));
        assert!(!mode.allows(Some(PARAMDEF_DEFAULT_ENDURANCE)));
        assert!(!mode.allows(Some(90.0)));
    }

    #[test]
    fn an_unknown_duration_is_never_guessed_at() {
        // No master catalog on disk: neither mode may pretend to know.
        assert!(
            PermanentEffects::Exclude.allows(None),
            "exclude keeps what it cannot prove permanent"
        );
        assert!(
            !PermanentEffects::Only.allows(None),
            "only drops what it cannot prove permanent"
        );
    }

    #[test]
    fn a_missing_endurance_field_reads_as_zero_not_as_permanent() {
        // The catalog omits fields equal to the PARAMDEF default, and that default is 0.
        assert!(!is_permanent(PARAMDEF_DEFAULT_ENDURANCE));
        assert!(PermanentEffects::Exclude.allows(Some(PARAMDEF_DEFAULT_ENDURANCE)));
    }

    #[test]
    fn the_setting_parses_its_words_and_rejects_nonsense() {
        assert_eq!(
            PermanentEffects::parse("include"),
            Some(PermanentEffects::Include)
        );
        assert_eq!(
            PermanentEffects::parse(" EXCLUDE "),
            Some(PermanentEffects::Exclude)
        );
        assert_eq!(
            PermanentEffects::parse("Only"),
            Some(PermanentEffects::Only)
        );
        assert_eq!(
            PermanentEffects::parse("permanent"),
            Some(PermanentEffects::Only)
        );
        assert_eq!(
            PermanentEffects::parse("temporary"),
            Some(PermanentEffects::Exclude)
        );
        assert_eq!(PermanentEffects::parse(""), None);
        assert_eq!(PermanentEffects::parse("maybe"), None);
    }

    #[test]
    fn every_mode_round_trips_through_its_written_form() {
        for mode in [
            PermanentEffects::Include,
            PermanentEffects::Exclude,
            PermanentEffects::Only,
        ] {
            assert_eq!(PermanentEffects::parse(mode.as_str()), Some(mode));
        }
    }
}
