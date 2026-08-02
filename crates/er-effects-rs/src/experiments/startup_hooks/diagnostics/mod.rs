// Runtime diagnostic detours with no product feature ownership.
//
// This module keeps a compatibility glob export at the parent boundary while the
// startup-hook include tree is converted from one flat namespace into owned modules.
// The included files still rely on the old parent scope during this transition.
#![allow(unused_imports)]

use super::*;

include!("msb_parse_trace.rs");
include!("loadlist_wait_trace.rs");
include!("dlc_roots_trace.rs");
