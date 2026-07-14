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

pub(crate) fn game_module_base() -> Result<usize, String> {
    let module = unsafe { GetModuleHandleA(PCSTR::null()) }
        .map_err(|error| format!("failed to resolve game module: {error}"))?;
    Ok(module.0 as usize)
}
pub(crate) fn game_rva(rva: u32) -> Result<usize, String> {
    Ok(game_module_base()? + rva as usize)
}
pub(crate) unsafe fn is_heap_aligned_ptr(ptr: usize) -> bool {
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    ptr >= HEAP_LO && (ptr & PTR_ALIGN_MASK) == TITLE_OWNER_SCAN_START_ADDRESS
}
pub(crate) fn vtable_in_game_image(vtable: usize, base: usize) -> bool {
    const MODULE_MIN_OFFSET: usize = 0x1000;
    const MODULE_SPAN_FALLBACK: usize = 0x3000000;
    vtable >= base + MODULE_MIN_OFFSET && vtable < base + MODULE_SPAN_FALLBACK
}
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
/// Fault-tolerant pointer-sized read via ReadProcessMemory: returns None on
/// unmapped/freed memory instead of raising an access violation. Used by the
/// title-owner scan to survive the TOCTOU race against the booting game.
pub(crate) unsafe fn safe_read_usize(addr: usize) -> Option<usize> {
    let mut value: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut usize as *mut c_void,
            std::mem::size_of::<usize>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<usize>() {
        Some(value)
    } else {
        None
    }
}
/// Fault-tolerant i32 read via ReadProcessMemory (None on unmapped memory).
pub(crate) unsafe fn safe_read_i32(addr: usize) -> Option<i32> {
    let mut value: i32 = TITLE_OWNER_SCAN_START_ADDRESS as i32;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut i32 as *mut c_void,
            std::mem::size_of::<i32>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<i32>() {
        Some(value)
    } else {
        None
    }
}
/// Fault-tolerant f32 read via ReadProcessMemory (None on unmapped memory).
pub(crate) unsafe fn safe_read_f32(addr: usize) -> Option<f32> {
    let mut value: f32 = 0.0;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut f32 as *mut c_void,
            std::mem::size_of::<f32>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<f32>() {
        Some(value)
    } else {
        None
    }
}
/// Fault-tolerant single-byte read via ReadProcessMemory (None on unmapped memory). Used by the
/// WorldBlockRes::Update diagnostic detour to sample the phase ([+0x35]) and gate ([+0x2f]) bytes
/// without ever dereferencing a raw pointer into possibly-unmapped block memory.
pub(crate) unsafe fn safe_read_u8(addr: usize) -> Option<u8> {
    let mut value: u8 = 0;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut u8 as *mut c_void,
            std::mem::size_of::<u8>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<u8>() {
        Some(value)
    } else {
        None
    }
}

pub(crate) unsafe fn safe_read_u16(addr: usize) -> Option<u16> {
    let mut value: u16 = 0;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut u16 as *mut c_void,
            std::mem::size_of::<u16>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<u16>() {
        Some(value)
    } else {
        None
    }
}
