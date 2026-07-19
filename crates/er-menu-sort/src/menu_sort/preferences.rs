use std::fmt;

use super::toml::TomlString;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuSortDefault {
    Preserve,
    ItemType,
    OrderOfAcquisition,
}

impl MenuSortDefault {
    pub(crate) fn parse_label(value: &str) -> Result<Self, &'static str> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "preserve" | "disabled" | "disable" | "off" | "none" | "vanilla" => Ok(Self::Preserve),
            "item_type" | "type" => Ok(Self::ItemType),
            "order_of_acquisition" | "acquisition" | "order_acquisition" | "acquired" => {
                Ok(Self::OrderOfAcquisition)
            }
            _ => Err("expected order_of_acquisition, item_type, or preserve"),
        }
    }

    pub(crate) fn parse_toml_value(value: &str) -> Result<Self, &'static str> {
        let label = TomlString::parse(value)?;
        Self::parse_label(&label)
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::ItemType => "item_type",
            Self::OrderOfAcquisition => "order_of_acquisition",
        }
    }
}

impl fmt::Display for MenuSortDefault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MenuSortPreferences {
    pub(crate) armaments: MenuSortDefault,
    pub(crate) armor: MenuSortDefault,
    pub(crate) talismans: MenuSortDefault,
}

impl MenuSortPreferences {
    pub(super) const fn categories(self) -> [SortCategory; 3] {
        [
            SortCategory::new(SortMenuKind::Armaments, self.armaments),
            SortCategory::new(SortMenuKind::Armor, self.armor),
            SortCategory::new(SortMenuKind::Talismans, self.talismans),
        ]
    }
}

impl Default for MenuSortPreferences {
    fn default() -> Self {
        Self {
            armaments: MenuSortDefault::OrderOfAcquisition,
            armor: MenuSortDefault::OrderOfAcquisition,
            talismans: MenuSortDefault::OrderOfAcquisition,
        }
    }
}

impl fmt::Display for MenuSortPreferences {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "armaments={} armor={} talismans={}",
            self.armaments, self.armor, self.talismans
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MenuSortPreferenceOverrides {
    pub(crate) armaments: Option<MenuSortDefault>,
    pub(crate) armor: Option<MenuSortDefault>,
    pub(crate) talismans: Option<MenuSortDefault>,
}

impl MenuSortPreferenceOverrides {
    pub(crate) fn resolve(self) -> MenuSortPreferences {
        let defaults = MenuSortPreferences::default();
        MenuSortPreferences {
            armaments: self.armaments.unwrap_or(defaults.armaments),
            armor: self.armor.unwrap_or(defaults.armor),
            talismans: self.talismans.unwrap_or(defaults.talismans),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SortCategory {
    kind: SortMenuKind,
    configured_default: MenuSortDefault,
}

impl SortCategory {
    const fn new(kind: SortMenuKind, configured_default: MenuSortDefault) -> Self {
        Self {
            kind,
            configured_default,
        }
    }

    pub(super) const fn kind(self) -> SortMenuKind {
        self.kind
    }

    pub(super) const fn configured_default(self) -> MenuSortDefault {
        self.configured_default
    }
}

impl fmt::Display for SortCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind.label())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SortMenuKind {
    Armaments,
    Armor,
    Talismans,
}

impl SortMenuKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Armaments => "armaments",
            Self::Armor => "armor",
            Self::Talismans => "talismans",
        }
    }

    pub(super) const fn state_index(self) -> usize {
        match self {
            Self::Armaments => 4,
            Self::Armor => 6,
            Self::Talismans => 9,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sort_aliases() {
        assert_eq!(
            MenuSortDefault::parse_label("order-of-acquisition"),
            Ok(MenuSortDefault::OrderOfAcquisition)
        );
        assert_eq!(
            MenuSortDefault::parse_label("type"),
            Ok(MenuSortDefault::ItemType)
        );
        assert_eq!(
            MenuSortDefault::parse_label("vanilla"),
            Ok(MenuSortDefault::Preserve)
        );
    }

    #[test]
    fn resolves_partial_overrides_over_defaults() {
        let overrides = MenuSortPreferenceOverrides {
            armaments: Some(MenuSortDefault::ItemType),
            armor: None,
            talismans: Some(MenuSortDefault::Preserve),
        };

        assert_eq!(
            overrides.resolve(),
            MenuSortPreferences {
                armaments: MenuSortDefault::ItemType,
                armor: MenuSortDefault::OrderOfAcquisition,
                talismans: MenuSortDefault::Preserve,
            }
        );
    }
}
