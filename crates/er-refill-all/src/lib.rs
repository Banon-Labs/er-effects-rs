//! Mark every refillable item refill / no-refill, on one controller or keyboard combination,
//! and only inside the storage box.
//!
//! Elden Ring's storage box lets you flag items to be topped back up from storage at a site of
//! grace, one item at a time. With a few hundred flagged items that is a few hundred presses. This
//! DLL does the whole set at once, and cycles: press once for "everything refills", press again
//! for "nothing refills".
//!
//! Three things are worth knowing before reading further, each of which is a way this could look
//! like it works and not:
//!
//! * **The obvious native function is a toggle, not a setter.** Looping it would scatter the
//!   states rather than set them. See [`mark`].
//! * **The state vector is fixed at 2048 entries and overflowing it crashes the game.** See
//!   [`mark::INSERT_CEILING`].
//! * **The storage-box restriction is structural.** This code runs from inside the storage box
//!   dialog's own update, so "not in the storage box" means "never called". See [`runtime`].
//!
//! Item eligibility, stack limits, destination capacity and the storage -> inventory transfer are
//! all delegated to the game's own 1.16.2 code rather than reimplemented.

// Ungated on purpose: pure text/bit parsing, so their tests run on the host.
mod config;
mod log;
mod mark;

#[cfg(windows)]
mod runtime;

/// Only `DllMain` reads this, and `DllMain` only exists on Windows.
#[cfg(windows)]
const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // One sink for this DLL's hook + address lines. Without it a refused address is
        // silent here, because every cdylib links its own copy of er-hook/er-game-base.
        // A rust_panic in a cdylib loaded into the game is otherwise anonymous: the message goes to a
        // stderr nobody reads, and what survives is a 0xe06d7363 record naming the module and nothing
        // else. Two boots were lost to one before this existed. See er_game_base::panic_report.
        er_game_base::panic_report::report_panics_to("er-refill-all", crate::log::refill_log);
        er_hook::set_hook_logger(crate::log::refill_log);
        let module_base = module as usize;
        START.call_once(|| runtime::spawn(module_base));
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_refill_all_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
