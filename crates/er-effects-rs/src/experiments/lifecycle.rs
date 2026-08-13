//! Runtime lifecycle seams for attach-time experiment hook installation.
//!
//! Keep hook ordering here behavior-preserving: these functions are thin orchestration
//! wrappers around code that previously lived inline in `DllMain`.

use super::*;

mod save_flow;
pub(crate) use save_flow::*;

mod task_tick;
pub(crate) use task_tick::*;

mod title_visual_startup;
pub(crate) use title_visual_startup::*;

mod hook_installers;
pub(crate) use hook_installers::*;
