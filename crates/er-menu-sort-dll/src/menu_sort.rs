use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    log::Log,
    win::{GameAddress, GameModule, ProcessMemory},
};

const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
const GAME_DATA_MAN_MENU_SAVELOAD_OFFSET: usize = 0x60;
const SORT_STATE_ARRAY_OFFSET: usize = 0x1440;
const SORT_STATE_ENTRY_SIZE: usize = 4;
const SORT_STATE_REVERSE_FLAG: u32 = 0x8000_0000;
const SORT_STATE_ID_MASK: u32 = 0x7fff_ffff;

const ITEM_TYPE: SortCriterion = SortCriterion(0x5141);
const ORDER_OF_ACQUISITION: SortCriterion = SortCriterion(0x5140);

static DEFAULTS_APPLIED: AtomicBool = AtomicBool::new(false);

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

    fn target_state(self) -> Option<SortStateValue> {
        match self {
            Self::Preserve => None,
            Self::ItemType => Some(SortStateValue::reversed(ITEM_TYPE)),
            Self::OrderOfAcquisition => Some(SortStateValue::reversed(ORDER_OF_ACQUISITION)),
        }
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
    pub(crate) const fn categories(self) -> [SortCategory; 3] {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

impl Default for MenuSortPreferenceOverrides {
    fn default() -> Self {
        Self {
            armaments: None,
            armor: None,
            talismans: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SortCategory {
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

    fn target_state(self) -> Option<SortStateValue> {
        self.configured_default.target_state()
    }
}

impl fmt::Display for SortCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind.label())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortMenuKind {
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

    const fn state_index(self) -> usize {
        match self {
            Self::Armaments => 4,
            Self::Armor => 6,
            Self::Talismans => 9,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SortCriterion(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SortStateValue(u32);

impl SortStateValue {
    const fn reversed(criterion: SortCriterion) -> Self {
        Self(SORT_STATE_REVERSE_FLAG | criterion.0)
    }

    const fn criterion(self) -> SortCriterion {
        SortCriterion(self.0 & SORT_STATE_ID_MASK)
    }

    const fn raw(self) -> u32 {
        self.0
    }
}

impl From<i32> for SortStateValue {
    fn from(value: i32) -> Self {
        Self(value as u32)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MenuSortApplier {
    preferences: MenuSortPreferences,
    memory: ProcessMemory,
}

impl MenuSortApplier {
    pub(crate) fn new(preferences: MenuSortPreferences) -> Self {
        Self {
            preferences,
            memory: ProcessMemory,
        }
    }

    pub(crate) fn apply_once(self) {
        if DEFAULTS_APPLIED.load(Ordering::SeqCst) {
            return;
        }

        let Some(menu_state) = MenuSystemSaveLoad::resolve(self.memory) else {
            return;
        };
        let Some(report) = self.apply_to(menu_state) else {
            return;
        };

        DEFAULTS_APPLIED.store(true, Ordering::SeqCst);
        Log::write(format_args!(
            "menu-sort-dll: applied defaults mss=0x{:x} changed={} already={} skipped={} preferences={}",
            menu_state.address().value(),
            report.changed,
            report.already,
            report.skipped,
            self.preferences
        ));
    }

    fn apply_to(self, menu_state: MenuSystemSaveLoad) -> Option<ApplyReport> {
        let mut report = ApplyReport::default();
        for category in self.preferences.categories() {
            report.record(self.apply_category(menu_state, category)?);
        }
        Some(report)
    }

    fn apply_category(
        self,
        menu_state: MenuSystemSaveLoad,
        category: SortCategory,
    ) -> Option<CategoryApplyOutcome> {
        let Some(target_state) = category.target_state() else {
            Log::write(format_args!(
                "menu-sort-dll: preserve configured category={category}"
            ));
            return Some(CategoryApplyOutcome::Skipped);
        };

        let slot = menu_state.sort_slot(category.kind);
        let current_state = slot.read(self.memory)?;
        if current_state.criterion() == target_state.criterion() {
            return Some(CategoryApplyOutcome::Already);
        }
        if current_state.criterion() != ITEM_TYPE {
            Log::write(format_args!(
                "menu-sort-dll: preserve user/non-item category={category} value=0x{:x} configured={}",
                current_state.raw(),
                category.configured_default
            ));
            return Some(CategoryApplyOutcome::Skipped);
        }
        if slot.write(self.memory, target_state) {
            Some(CategoryApplyOutcome::Changed)
        } else {
            Log::write(format_args!(
                "menu-sort-dll: deferred; could not write category={category} addr=0x{:x}",
                slot.address().value()
            ));
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ApplyReport {
    changed: usize,
    already: usize,
    skipped: usize,
}

impl ApplyReport {
    fn record(&mut self, outcome: CategoryApplyOutcome) {
        match outcome {
            CategoryApplyOutcome::Changed => self.changed += 1,
            CategoryApplyOutcome::Already => self.already += 1,
            CategoryApplyOutcome::Skipped => self.skipped += 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CategoryApplyOutcome {
    Changed,
    Already,
    Skipped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MenuSystemSaveLoad {
    address: GameAddress,
}

impl MenuSystemSaveLoad {
    fn resolve(memory: ProcessMemory) -> Option<Self> {
        let game = GameModule::current().ok()?;
        let game_data_man = memory.read_address(game.rva(GAME_DATA_MAN_GLOBAL_RVA))?;
        let address =
            memory.read_address(game_data_man.offset(GAME_DATA_MAN_MENU_SAVELOAD_OFFSET))?;
        Some(Self { address })
    }

    const fn address(self) -> GameAddress {
        self.address
    }

    const fn sort_slot(self, kind: SortMenuKind) -> SortStateSlot {
        SortStateSlot {
            address: self
                .address
                .offset(SORT_STATE_ARRAY_OFFSET + kind.state_index() * SORT_STATE_ENTRY_SIZE),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SortStateSlot {
    address: GameAddress,
}

impl SortStateSlot {
    const fn address(self) -> GameAddress {
        self.address
    }

    fn read(self, memory: ProcessMemory) -> Option<SortStateValue> {
        let state = memory.read_i32(self.address).map(SortStateValue::from);
        if state.is_none() {
            Log::write(format_args!(
                "menu-sort-dll: deferred; could not read addr=0x{:x}",
                self.address.value()
            ));
        }
        state
    }

    fn write(self, memory: ProcessMemory, value: SortStateValue) -> bool {
        memory.write_u32(self.address, value.raw())
    }
}

struct TomlString;

impl TomlString {
    fn parse(value: &str) -> Result<String, &'static str> {
        let value = value.trim();
        if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
            return Ok(value[1..value.len() - 1].to_owned());
        }
        if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
            return Err("expected a quoted TOML string");
        }

        let mut parsed = String::with_capacity(value.len());
        let mut chars = value[1..value.len() - 1].chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('\\') => parsed.push('\\'),
                    Some('"') => parsed.push('"'),
                    Some('n') => parsed.push('\n'),
                    Some('r') => parsed.push('\r'),
                    Some('t') => parsed.push('\t'),
                    _ => return Err("unsupported escape in string"),
                }
            } else {
                parsed.push(ch);
            }
        }
        Ok(parsed)
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
