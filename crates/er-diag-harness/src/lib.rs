//! er-diag-harness -- standalone Elden Ring agent diagnostic trace harness.
//!
//! A separate cdylib (`er_diag_harness.dll`), loaded as its own `[[natives]]` entry in the ME3
//! profile alongside the product. Its mere presence enables it (default-on, no env var and no
//! marker file, the `er-input-harness` contract); omit it from the profile for production.
//!
//! Why it exists. Five MinHook detours -- three traces' worth -- used to be installed
//! unconditionally by the shipping `er_quickload.dll`, from `DllMain` ->
//! `install_profile_and_system_quit_hooks` -> `install_system_quit_duplicate_button_hook`, with no
//! gate of any kind:
//!
//! | address    | what it is                                   | how often it fires        |
//! |------------|----------------------------------------------|---------------------------|
//! | `0x21bbf0` | the sole `MsbFileCap::msbResCap` writer       | once per MSB, every boot  |
//! | `0xaf1800` | `CS::MoveMapListStep::STEP_LoadListWait`      | every frame               |
//! | `0xe06490` | DLC virtual-root blank (`FUN_140e06490`)      | per title start-game pass |
//! | `0xe05fb0` | DLC virtual-root refill (`FUN_140e05fb0`)     | per refill attempt        |
//! | `0x836f30` | the refill job body (`FUN_140836f30`)         | per job dispatch          |
//!
//! Every one of them is observe-and-forward, and none of them feeds an `oracle_*` field -- so
//! nothing in the product read the results back either. They were pure agent diagnostics riding in
//! a player's process. They now ride here.
//!
//! Cross-DLL STATE: separate DLLs do not share Rust statics, so this shell carries its own counters
//! and its own trampoline slots and re-derives everything else by reading game memory directly
//! (`er_game_base::filecap`). It reads no product static and calls no product function. The one
//! place the old arrangement leaked across that line is documented in `dlc_roots_trace.rs`.
//!
//! MINHOOK is per-process, the guards are per-DLL. `MH_Initialize` / `MH_ApplyQueued` operate on
//! one process-wide MinHook state, but the `*_INSTALLED` atomics below are this image's own, so
//! they cannot deduplicate against the product's. That is safe only because the product no longer
//! hooks any of these five addresses -- which is the whole point of the move, and is checked by
//! `scripts/check-shared-hook-rvas.py`.

// Everything below the entry point is Windows-only by construction -- a MinHook detour on a game
// RVA, a walk over live game memory, a log file beside `eldenring.exe`. So the modules are
// `cfg(windows)` rather than compiled-with-their-callers-removed, which is what the sibling shells
// need the blanket `cfg_attr(not(windows), allow(dead_code, unused_imports))` for. Here the host
// build is the `DllMain` stub and nothing else, and it is warning-clean under the workspace
// `[workspace.lints.rust] warnings = "deny"` -- so no allow is carried, and real host debt would
// show up instead of being hidden.
#[cfg(windows)]
mod dlc_roots_trace;
#[cfg(windows)]
mod loadlist_wait_trace;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod msb_parse_trace;
#[cfg(windows)]
mod rva;

#[cfg(windows)]
use std::sync::Once;

#[cfg(windows)]
use er_hook::{MH_ApplyQueued, MH_STATUS};
#[cfg(windows)]
use windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};

#[cfg(windows)]
use crate::log::diag_log;

const DLL_MAIN_SUCCESS: i32 = 1;

#[cfg(windows)]
static START: Once = Once::new();

/// Resolve the game image, queue every trace detour, then apply the queue once.
///
/// The three traces only `queue_enable`; inside the product they inherited a shared
/// `MH_ApplyQueued` from the System>Quit installer that happened to run after them. Nothing here
/// runs after them, so the apply is explicit -- without it all five detours would be created,
/// reported installed, and never fire.
#[cfg(windows)]
fn install() {
    log::reset_log_file();
    diag_log!(
        "er-diag-harness attach: msb-parse (0x21bbf0), STEP_LoadListWait (0xaf1800) and DLC virtual-root blank/refill/job (0xe06490/0xe05fb0/0x836f30) traces -- all observe-and-forward, evicted from the product DLL where they installed unconditionally"
    );
    // Wait for the game image to be mapped before resolving any RVA. No sleep: yield + re-poll,
    // the product's own wait pattern. `game_module_base` is a PE-header read, not a loader call,
    // so this is safe off the loader lock.
    // Bounded (2026-08-29): see er_game_base::wait -- an unbounded `loop { yield_now() }` starved
    // the wineserver and hung a whole boot.
    let Some(base) = er_game_base::wait::poll_until(|| {
        er_game_base::mem::game_module_base()
            .ok()
            .filter(|base| *base != 0)
    }) else {
        diag_log!("er-diag-harness: no game module base; this shell stays inert");
        return;
    };
    diag_log!("er-diag-harness: game module base 0x{base:x}");

    msb_parse_trace::install_msb_parse_trace();
    loadlist_wait_trace::install_loadlist_wait_trace();
    dlc_roots_trace::install_dlc_roots_trace();

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => diag_log!("er-diag-harness: MH_ApplyQueued ok -- traces are live"),
        status => diag_log!(
            "er-diag-harness: MH_ApplyQueued failed: {status:?} -- the queued traces are NOT live"
        ),
    }
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
        er_game_base::panic_report::report_panics_to("er-diag-harness", crate::log::log_line);
        er_hook::set_hook_logger(crate::log::log_line);
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-diag-harness-install".to_owned())
                .spawn(install);
        });
    }
    DLL_MAIN_SUCCESS
}

// Non-windows: keep the crate buildable for host tooling / workspace resolution.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_diag_harness_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
