// Runtime diagnostic detours with no product feature ownership.
// Shared imports preserved from the former flat startup-hook namespace for child modules.
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::*;
use crate::{crashlog::*, ffi::*, telemetry::*};
use eldenring::cs::PlayerIns;
use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(crate) mod msb_parse_trace;
pub(crate) use msb_parse_trace::*;

pub(crate) mod loadlist_wait_trace;
pub(crate) use loadlist_wait_trace::*;

pub(crate) mod dlc_roots_trace;
pub(crate) use dlc_roots_trace::*;

pub(crate) mod layout_global_hooks;
pub(crate) use layout_global_hooks::*;
