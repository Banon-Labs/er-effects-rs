//! er-input-harness -- standalone Elden Ring INPUT SELF-DRIVE HARNESS.
//!
//! A separate cdylib (`er_input_harness.dll`), loaded as its own `[[natives]]` entry in the ME3
//! profile alongside the product (or with the telemetry-only DLL for a vanilla capture). Its mere
//! presence enables it (default-on, no env/marker gate); omit it from the profile for production.
//!
//! MECHANISM: ER input is driven by writing the game's own input memory -- the CSMenuMan keystate
//! bitmap (`inputmgr+0x90+eventId`), the DLUID input-active flag (`+0x88d`), and the title global
//! accept byte (`base+0x4589bdc`) -- on the game thread each frame. SendInput/XInput/window-focus was a
//! dead path and is not carried over.
//!
//! Title-active hook (2026-07-22): the per-frame callback is a `CSTaskImp` `FrameBegin` recurring task
//! (same registration as er-telemetry), which fires at the title and boot screens and in-world --
//! unlike the previous in-world-only union MinHook anchor, which never ran at the title and so could not
//! drive press any button / Continue. This lets the harness drive the full native boot+reload standalone
//! (bd user-chose-build-harness-title-drive-cstaskimp-hook-accept-byte-2026-07-22).
//!
//! Cross-DLL STATE: separate DLLs do not share Rust statics, so this DLL re-derives menu/game state by
//! reading game memory directly (game_mem) instead of reading product statics.

// A cdylib whose every consumer is `DllMain` and the per-frame game task it registers, all of
// them `#[cfg(windows)]`. On a host build the shell is compiled with its only callers cfg'd out,
// so `dead_code`/`unused_imports` there report the cfg, not real debt (measured 2026-08-21: 108
// on the host, 0 on the shipping target). The shipping target carries the full deny.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

#[cfg(windows)]
mod drive;
mod game_mem;
mod input_inject;
mod key_inject;
mod log;
mod menu_query;
#[cfg(windows)]
mod pad_inject;
mod repl;
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
        "er-input-harness attach: TITLE-ACTIVE CSTaskImp FrameBegin self-drive (fires at title + in-world); direct input-memory injection (keystate bitmap + DLUID + accept byte); no SendInput/XInput"
    );
    // Wait for the game's task manager (no sleep: yield + re-poll, the product's wait pattern).
    // Bounded (2026-08-29): see er_game_base::wait -- the unbounded form of this loop starved
    // the wineserver and hung a boot.
    let Some(task) = er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
    else {
        return;
    };
    let base = resolve_base();
    input_inject::log_resolution(base);
    // Install the FD4PadDevice::poll MinHook so the in-world menu can be driven by the raw pad snapshot
    // (inputmgr+0x90 is output in-world; the menu reads the pad device). Title/boot still use the accept
    // byte from the CSTaskImp task below.
    pad_inject::install_pad_poll_hook(base);
    harness_log!("er-input-harness install complete {}", game_mem::snapshot());
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
        // One sink for this DLL's hook + address lines. Without it a refused address is
        // silent here, because every cdylib links its own copy of er-hook/er-game-base.
        // A rust_panic in a cdylib loaded into the game is otherwise anonymous: the message goes to a
        // stderr nobody reads, and what survives is a 0xe06d7363 record naming the module and nothing
        // else. Two boots were lost to one before this existed. See er_game_base::panic_report.
        er_game_base::panic_report::report_panics_to("er-input-harness", crate::log::log_line);
        er_hook::set_hook_logger(crate::log::log_line);
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
pub extern "C" fn er_input_harness_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
