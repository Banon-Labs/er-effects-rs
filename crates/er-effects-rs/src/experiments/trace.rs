//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

mod menu_trace_hooks;
pub(crate) use menu_trace_hooks::*;

mod menu_constructor_capture;
pub(crate) use menu_constructor_capture::*;

mod native_result_map_hooks;
pub(crate) use native_result_map_hooks::*;
