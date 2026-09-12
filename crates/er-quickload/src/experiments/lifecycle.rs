//! Runtime lifecycle seams for attach-time experiment hook installation.
//!
//! Keep hook ordering here behavior-preserving: these functions are thin orchestration
//! wrappers around code that previously lived inline in `DllMain`.

use super::*;

// The install-once primitive the save-flow boxes retry through, now shared out of `crate::mh`.
use crate::mh::mh_install_hook_once;

mod save_game_flow_handlers;
pub(crate) use save_game_flow_handlers::*;

mod save_dest_commit;
pub(crate) use save_dest_commit::*;

mod save_flow_boxes;
pub(crate) use save_flow_boxes::*;

mod save_flow;
pub(crate) use save_flow::*;

mod task_tick;
pub(crate) use task_tick::*;

mod title_visual_startup;
pub(crate) use title_visual_startup::*;

mod hook_installers;
pub(crate) use hook_installers::*;
