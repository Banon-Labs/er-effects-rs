//! er-input-harness-dll -- standalone Elden Ring INPUT SELF-DRIVE HARNESS.
//!
//! A separate cdylib (`er_input_harness_dll.dll`), loaded as its own `[[natives]]` entry in the ME3
//! profile ALONGSIDE the product (or with the telemetry-only DLL for a vanilla capture). Its mere
//! PRESENCE enables it (DEFAULT-ON, no env/marker gate); omit it from the profile for production.
//!
//! MECHANISM: ER input is driven by writing the game's OWN input memory -- the CSMenuMan keystate
//! bitmap (`inputmgr+0x90+eventId`), the DLUID input-active flag (`+0x88d`), and the title global
//! accept byte (`base+0x4589bdc`) -- on the GAME THREAD each frame. SendInput/XInput/window-focus was a
//! DEAD path and is not carried over.
//!
//! TITLE-ACTIVE HOOK (2026-07-22): the per-frame callback is a `CSTaskImp` `FrameBegin` recurring task
//! (same registration as er-telemetry-dll), which fires at the TITLE and boot screens AND in-world --
//! unlike the previous in-world-only union MinHook anchor, which never ran at the title and so could not
//! drive PRESS ANY BUTTON / Continue. This lets the harness drive the FULL native boot+reload standalone
//! (bd USER-chose-build-harness-title-drive-cstaskimp-hook-accept-byte-2026-07-22).
//!
//! CROSS-DLL STATE: separate DLLs do not share Rust statics, so this DLL re-derives menu/game state by
//! reading GAME memory directly (game_mem) instead of reading product statics.

#![allow(clippy::missing_safety_doc)]
#![allow(non_snake_case)]

#[cfg(windows)]
mod drive;
mod game_mem;
mod input_inject;
mod log;
#[cfg(windows)]
mod title_scan;
mod win32;

use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(windows)]
use std::sync::Once;

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
#[cfg(windows)]
use windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};

use crate::log::{harness_log, reset_log_file, reset_phases_file};

const DLL_MAIN_SUCCESS: i32 = 1;

/// Cached game image base, resolved once the game image is mapped.
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);

/// Resolve (and cache) the game image base; 0 until the image is mapped.
fn resolve_base() -> usize {
    let cached = GAME_BASE.load(Ordering::SeqCst);
    if cached != 0 {
        return cached;
    }
    let base = game_mem::game_base().unwrap_or(0);
    if base != 0 {
        GAME_BASE.store(base, Ordering::SeqCst);
    }
    base
}

#[cfg(windows)]
static START: Once = Once::new();

#[cfg(windows)]
fn install() {
    reset_log_file();
    reset_phases_file();
    harness_log!(
        "er-input-harness-dll attach: TITLE-ACTIVE CSTaskImp FrameBegin self-drive (fires at title + in-world); direct input-memory injection (keystate bitmap + DLUID + accept byte); no SendInput/XInput"
    );
    // Wait for the game's task manager (no sleep: yield + re-poll, the product's wait pattern).
    let task = loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(t) => break t,
            Err(_) => std::thread::yield_now(),
        }
    };
    let base = resolve_base();
    input_inject::log_resolution(base);
    harness_log!(
        "er-input-harness-dll install complete {}",
        game_mem::snapshot()
    );
    task.run_recurring(
        |_data: &FD4TaskData| {
            let base = resolve_base();
            if base != 0 {
                drive::on_frame(base);
            }
        },
        CSTaskGroupIndex::FrameBegin,
    );
}

/// # Safety
/// Standard `DllMain` contract. On attach it only spawns a thread (no loader-lock work).
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    _module: HINSTANCE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-input-harness-install".to_owned())
                .spawn(install);
        });
    }
    DLL_MAIN_SUCCESS
}

// Non-windows: keep the crate buildable for host tooling / workspace resolution.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_input_harness_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
