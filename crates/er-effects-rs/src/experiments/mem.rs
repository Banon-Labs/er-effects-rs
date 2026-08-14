//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
            WM_KEYUP,
        },
    },
    core::{BOOL, PCSTR},
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

// Fault-safe RAM readers + game base/rva/image primitives now live in the shared
// er-game-base crate (single source of truth across product + telemetry + the
// mini-DLLs). Re-exported at their historical `crate::experiments::` paths so the
// ~40 product/telemetry call sites (bare via glob and fully-qualified) are
// unchanged. patch_3byte_stub / apply_xor_ret_stub stay below: they depend on the
// windows crate + product constants + append_autoload_debug.
pub(crate) use er_game_base::mem::{
    game_module_base, game_rva, is_heap_aligned_ptr, safe_read_f32, safe_read_i32, safe_read_u8,
    safe_read_u16, safe_read_usize, vtable_in_game_image,
};

// PlayerGameData character-name layout/readers now live beside the shared fault-safe RAM readers.
// Keep the historical flat product names while the remaining experiments modules are extracted.
pub(crate) use er_game_base::pgd::{
    read_utf16_name_units, utf16_name_empty_like, utf16_names_equal,
};
// safe_read_usize/i32/f32/u8/u16 moved to er_game_base::mem (re-exported above).
