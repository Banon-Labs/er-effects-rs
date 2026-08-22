// Product (A) boot missing-save picker modules.
// Shared imports preserved from the former flat startup-hook namespace for child modules.
use crate::telemetry::*;
use crate::*;
use er_save_picker::os_dialog::{no_picker_cover, os_pick_validated};
use er_telemetry::counters::SAVE_PICKER_OPEN_COUNT;
use er_telemetry::counters::SAVE_PICKER_PICK_COUNT;
use er_telemetry::counters::SAVE_PICKER_SURFACE;
use std::{
    path::Path,
    sync::{OnceLock, atomic::Ordering},
    time::{Duration, Instant},
};

pub(crate) mod save_picker_os_dialog;
pub(crate) use save_picker_os_dialog::*;

pub(crate) mod save_picker_boot;
pub(crate) use save_picker_boot::*;

pub(crate) mod save_picker_surface;
pub(crate) use save_picker_surface::*;
