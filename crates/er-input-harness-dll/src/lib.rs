//! er-input-harness-dll -- standalone Elden Ring INPUT SELF-DRIVE HARNESS.
//!
//! A separate cdylib (`er_input_harness_dll.dll`), loaded as its own `[[natives]]` entry in the ME3
//! profile ALONGSIDE the product `er_effects_rs.dll`. Its mere PRESENCE enables it (DEFAULT-ON, no
//! env/marker gate); omit it from the profile for production. This realizes the architecture directive
//! `multi-dll-separate-crates-per-feature-single-me3-profile-2026-07-19`: an optional feature is a
//! separate crate -> separate DLL, gated by which DLLs a single ME3 profile lists, NOT by an
//! env/marker inside the product DLL.
//!
//! MECHANISM (user-corrected 2026-07-19): ER input is driven by writing the game's OWN input memory
//! -- the CSMenuMan keystate bitmap (`inputmgr+0x90+eventId`) and the DLUID input-active flag
//! (`+0x88d`) -- on the game thread each frame. The earlier SendInput/XInput/window-focus harness was
//! a DEAD PATH and is not carried over.
//!
//! CROSS-DLL STATE (constraint #1): separate DLLs do not share Rust statics, so this DLL re-derives
//! menu/game state by reading GAME memory directly (game_mem) instead of reading the product's
//! statics, and gets its game-thread per-frame callback by routing a hook through the product's
//! `er_effects_union_register` export (union) rather than sharing any product static. Where the drive
//! would trigger the product's quickload, it does so INDIRECTLY -- by driving the native menu, whose
//! transitions the PRODUCT's own ProfileSelect hooks observe to arm the reload.
//!
//! Structure mirrors the model crate `er-reload-trace-dll`: `[lib] crate-type=["cdylib"]`, a build.rs
//! MinHook cc build, and a `DllMain` that spawns an install thread (never blocking the loader lock).

#![allow(clippy::missing_safety_doc)]

mod drive;
mod game_mem;
mod input_inject;
mod log;
mod union;
mod win32;

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::log::{harness_log, reset_log_file};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

/// Cached game image base, set once the install thread resolves it.
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);

/// Per-frame anchor detour (union `HookFn` ABI). Chains the original, then runs one drive frame.
unsafe extern "system" fn drive_anchor_detour(a: usize, b: usize, c: usize, d: usize) -> usize {
    let ret = unsafe { union::call_anchor_original(a, b, c, d) };
    let base = GAME_BASE.load(Ordering::SeqCst);
    if base != 0 {
        drive::on_frame(base);
    }
    ret
}

fn install() {
    reset_log_file();
    harness_log!(
        "er-input-harness-dll attach: direct input-memory self-drive (CSMenuMan keystate bitmap + DLUID stay-active); no SendInput/XInput; game-thread-timed via product union hook"
    );
    // Wait briefly for the game image; `GetModuleHandleA(NULL)` is the primary module (eldenring.exe).
    let base = loop {
        if let Some(base) = game_mem::game_base() {
            break base;
        }
        unsafe { win32::Sleep(50) };
    };
    GAME_BASE.store(base, Ordering::SeqCst);
    input_inject::log_resolution(base);
    union::install_drive_hook(base, drive_anchor_detour);
    harness_log!(
        "er-input-harness-dll install complete {}",
        game_mem::snapshot()
    );
}

/// # Safety
/// Standard `DllMain` contract. On attach it only spawns a thread (no loader-lock work).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    _module: *mut c_void,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        let _ = std::thread::Builder::new()
            .name("er-input-harness-install".to_owned())
            .spawn(install);
    }
    DLL_MAIN_SUCCESS
}
