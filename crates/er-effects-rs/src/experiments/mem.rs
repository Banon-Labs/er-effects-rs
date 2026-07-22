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
            ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            WM_KEYDOWN, WM_KEYUP,
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

pub(crate) fn utf16_name_empty_like(units: &[u16], len: usize) -> bool {
    const NAME_LEN_NONE: usize = 0;
    const NAME_LEN_SINGLE: usize = 1;
    const NAME_UNDERSCORE: u16 = '_' as u16;
    const NAME_SPACE: u16 = ' ' as u16;
    if len == NAME_LEN_NONE {
        return true;
    }
    if len == NAME_LEN_SINGLE && units.first().copied() == Some(NAME_UNDERSCORE) {
        return true;
    }
    units.iter().take(len).all(|unit| *unit == NAME_SPACE)
}
pub(crate) fn utf16_names_equal(left: &[u16], right: &[u16], len: usize) -> bool {
    left.get(..len) == right.get(..len)
}
pub(crate) unsafe fn read_utf16_name_units(addr: usize) -> ([u16; PGD_NAME_LEN_U16], usize) {
    const ZERO_U16: u16 = 0;
    const U16_STRIDE: usize = 2;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    let mut units = [ZERO_U16; PGD_NAME_LEN_U16];
    let mut len = IDX_START;
    while len < PGD_NAME_LEN_U16 {
        let unit = unsafe { safe_read_usize(addr + len * U16_STRIDE) }
            .map(|value| value as u16)
            .unwrap_or(ZERO_U16);
        units[len] = unit;
        if unit == ZERO_U16 {
            break;
        }
        len += IDX_STEP;
    }
    (units, len)
}
/// Write a self-contained 3-byte return stub at `base+rva` after validating the expected first
/// byte. RWX via VirtualProtect, write, restore, icache flush. Returns true on success. Shared by
/// the gate-force patches (foreground / sign-in / user-index).
pub(crate) fn patch_3byte_stub(
    base: usize,
    rva: usize,
    expected_first: u8,
    stub: [u8; 3],
    label: &str,
) -> bool {
    let target = (base + rva) as *mut u8;
    let existing = unsafe { *target };
    if existing != expected_first {
        append_autoload_debug(format_args!(
            "{label}: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{expected_first:x}",
            base + rva
        ));
        return false;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!("{label}: VirtualProtect failed"));
        return false;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = stub[i] };
        i += ONLINE_DISABLE_BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            ONLINE_DISABLE_PATCH_LEN,
        )
    };
    true
}
/// Patch a 0x48-prologue function body to `xor eax,eax; ret` (return 0) at `base+rva`. Validates
/// the expected first byte, VirtualProtects RWX, writes the 3-byte stub, restores protection, and
/// flushes the icache. Used to force-offline the IsOnlineMode getter + login-readiness predicate.
pub(crate) fn apply_xor_ret_stub(base: usize, rva: usize, label: &str) {
    let target = (base + rva) as *mut u8;
    let existing = unsafe { *target };
    if existing != ONLINE_DISABLE_EXPECTED_FIRST {
        append_autoload_debug(format_args!(
            "online-disable: ABORT {label} -- byte at 0x{:x} is 0x{existing:x}, expected 0x{ONLINE_DISABLE_EXPECTED_FIRST:x}",
            base + rva
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!(
            "online-disable: VirtualProtect failed for {label}"
        ));
        return;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = ONLINE_DISABLE_STUB[i] };
        i += ONLINE_DISABLE_BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            ONLINE_DISABLE_PATCH_LEN,
        )
    };
    append_autoload_debug(format_args!(
        "online-disable: patched {label} 0x{:x} -> xor eax,eax;ret (forces offline)",
        base + rva
    ));
}
// safe_read_usize/i32/f32/u8/u16 moved to er_game_base::mem (re-exported above).
