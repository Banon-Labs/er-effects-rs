use std::ffi::c_void;

use crate::{config::RuntimeConfig, log::Log, task::MenuSortTask};

mod config;
mod log;
mod menu_sort;
mod process_memory;
mod task;

const DLL_MAIN_SUCCESS: i32 = 1;
const DLL_PROCESS_ATTACH: u32 = 1;

type HInstance = *mut c_void;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_hmodule: HInstance, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        RuntimeConfig::install();
        Log::write(format_args!(
            "menu-sort: attach defaults={}",
            RuntimeConfig::active_preferences()
        ));
        MenuSortTask::start_once();
    }

    DLL_MAIN_SUCCESS
}
