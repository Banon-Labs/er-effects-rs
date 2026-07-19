use std::sync::Once;

use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};

use crate::{config::RuntimeConfig, log::Log, menu_sort::MenuSortApplier};

static START_MENU_SORT_TASK: Once = Once::new();

pub(crate) struct MenuSortTask;

impl MenuSortTask {
    pub(crate) fn start_once() {
        START_MENU_SORT_TASK.call_once(TaskThread::spawn);
    }
}

struct TaskThread;

impl TaskThread {
    fn spawn() {
        let _ = std::thread::Builder::new()
            .name("er-menu-sort-task".to_owned())
            .spawn(|| TaskRegistration::new(RuntimeConfig::active_preferences()).register());
    }
}

#[derive(Clone, Copy, Debug)]
struct TaskRegistration {
    applier: MenuSortApplier,
}

impl TaskRegistration {
    fn new(preferences: crate::menu_sort::MenuSortPreferences) -> Self {
        Self {
            applier: MenuSortApplier::new(preferences),
        }
    }

    fn register(self) {
        let cs_task = TaskInstance::wait();
        Log::write(format_args!(
            "menu-sort: CSTaskImp ready; registering recurring task"
        ));
        cs_task.run_recurring(
            move |_task_data: &FD4TaskData| self.applier.apply_once(),
            CSTaskGroupIndex::FrameBegin,
        );
    }
}

struct TaskInstance;

impl TaskInstance {
    fn wait() -> &'static CSTaskImp {
        Log::write(format_args!("menu-sort: waiting for CSTaskImp"));
        loop {
            if let Some(instance) = Self::try_current() {
                return instance;
            }
            std::thread::yield_now();
        }
    }

    fn try_current() -> Option<&'static CSTaskImp> {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => Some(instance),
            Err(InstanceError::NotFound(_)) | Err(InstanceError::Null(_)) => None,
        }
    }
}
