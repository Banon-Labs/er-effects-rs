// Runtime diagnostic detours with no product feature ownership.
// Shared imports preserved from the former flat startup-hook namespace for child modules.
//
// Three traces left this module on 2026-08-25, into the `er-diag-harness` cdylib
// (`crates/er-diag-harness/`): `msb_parse_trace`, `loadlist_wait_trace` and `dlc_roots_trace`.
// All three were installed unconditionally at process attach -- `install_system_quit_duplicate_button_hook`
// called them with no gate -- and all three are observe-and-forward with no `oracle_*` export, so
// the shipping DLL was detouring the sole `msbResCap` writer, a per-frame map step and the three
// DLC virtual-root entry points purely so an agent could read a log. No facade is left for them:
// nothing outside this directory ever named them, and a stub that silently installed nothing would
// be a worse lie than the missing symbol.
//
// `layout_global_hooks` stays. Despite living under `diagnostics/`, it owns real product surface --
// the System>Quit cloned rows, the splash-skip patch and the pre-world Wwise mute -- and
// `bootstrap.rs` calls into it.
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::*;
use crate::{crashlog::*, ffi::*, telemetry::*};
use eldenring::cs::PlayerIns;
use std::{ffi::c_void, sync::atomic::Ordering};

pub(crate) mod system_quit_ownership_repro;
pub(crate) use system_quit_ownership_repro::*;

pub(crate) mod layout_global_hooks;
pub(crate) use layout_global_hooks::*;
