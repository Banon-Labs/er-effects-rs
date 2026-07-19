use std::sync::atomic::{AtomicBool, Ordering};

use crate::{log::Log, process_memory::ProcessMemory};

use super::{
    preferences::{MenuSortPreferences, SortCategory},
    state::{ITEM_TYPE, MenuSystemSaveLoad, SortStateValue},
};

static DEFAULTS_APPLIED: AtomicBool = AtomicBool::new(false);

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
            "menu-sort: applied defaults mss=0x{:x} changed={} already={} skipped={} preferences={}",
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
        let Some(target_state) = SortStateValue::for_default(category.configured_default()) else {
            Log::write(format_args!(
                "menu-sort: preserve configured category={category}"
            ));
            return Some(CategoryApplyOutcome::Skipped);
        };

        let slot = menu_state.sort_slot(category.kind());
        let current_state = slot.read(self.memory)?;
        if current_state.criterion() == target_state.criterion() {
            return Some(CategoryApplyOutcome::Already);
        }
        if current_state.criterion() != ITEM_TYPE {
            Log::write(format_args!(
                "menu-sort: preserve user/non-item category={category} value=0x{:x} configured={}",
                current_state.raw(),
                category.configured_default()
            ));
            return Some(CategoryApplyOutcome::Skipped);
        }
        if slot.write(self.memory, target_state) {
            Some(CategoryApplyOutcome::Changed)
        } else {
            Log::write(format_args!(
                "menu-sort: deferred; could not write category={category} addr=0x{:x}",
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
