//! The build watermark as a mod of its own.
//!
//! # Why it is its own DLL
//!
//! The watermark answers "which builds are in this process, and is any of them behind `main`" --
//! a question about the whole profile, not about any one feature. Living inside a feature DLL
//! made it hostage to that feature: it appeared only in profiles that happened to load
//! `er-net-effects`, and a profile of one unrelated mod got no watermark at all. As its own
//! `[[natives]]` entry it is opt-in per profile, ships on its own, and has no opinion about what
//! else is loaded -- it reports whatever answers `er_build_identity_v1`, which every DLL in this
//! workspace exports whether or not it knows this shell exists.
//!
//! # What it does not do
//!
//! No detours, no game memory writes, no input. It installs one hudhook overlay, reads the
//! module list, and draws text. If another module in the process already owns the overlay, this
//! one stands down and draws nothing rather than installing a second imgui on the same swapchain.

#[cfg(windows)]
use std::sync::Once;

/// `DLL_PROCESS_ATTACH`.
#[cfg(windows)]
const DLL_PROCESS_ATTACH: u32 = 1;

/// `DllMain` must return true or the loader unloads us.
#[cfg(windows)]
const DLL_MAIN_SUCCESS: i32 = 1;

/// Log file name, beside the game executable like every other shell in this repo.
#[cfg(windows)]
const LOG_FILE_NAME: &str = "er-build-watermark.log";

#[cfg(windows)]
static START: Once = Once::new();

#[cfg(windows)]
fn log_path() -> std::path::PathBuf {
    er_game_base::log::game_directory_path()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(LOG_FILE_NAME)
}

#[cfg(windows)]
fn watermark_log(args: std::fmt::Arguments<'_>) {
    er_game_base::log::append_line(&log_path(), format_args!("er-build-watermark: {args}"));
}

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
        let module_base = module as usize;
        START.call_once(|| {
            // Before anything that can panic. The renderer this shell installs compiles its
            // shaders at first Present, inside a callback the game owns, and a failure there
            // unwinds straight through to an abort: on 2026-08-29 that killed a boot 229 ms
            // after the first backbuffer draw and left nothing on disk but the module name.
            er_game_base::panic_report::report_panics_to("er-build-watermark", watermark_log);
            watermark_log(format_args!(
                "loaded module_base=0x{module_base:x}; standalone build-watermark shell (no \
                 detours, no game writes)"
            ));
            // Off the loader thread: hudhook's install takes locks and enumerates modules, and
            // neither belongs inside `DllMain` where the loader lock is held.
            let spawned = std::thread::Builder::new()
                .name("er-build-watermark-install".to_string())
                .spawn(move || {
                    // Claim immediately, and let load order fall where it may. The first cut
                    // slept six seconds to yield the imgui context to a module with a richer UI;
                    // that sleep was removed for being synchronization, which
                    // `scripts/check-no-timeouts.py` rightly rejects, on the strength of a
                    // measurement that said both overlays rendered side by side. That
                    // measurement was wrong. On 2026-08-25 the user's live session had this
                    // shell logging `first render display_width=3840 rows=14` while
                    // `er-net-effects` sat at `hudhook_render_count = 0` -- installed, never
                    // rendered, no error logged anywhere -- and their interactive bar had been
                    // invisible since #336 added this shell. Two `Hudhook::apply()` calls in one
                    // process really do double-hook `Present`, and the second one loses.
                    //
                    // Neither the sleep nor the eager claim is needed now: `overlay_host` makes
                    // whoever arrives first the host and everyone else a guest that draws through
                    // it, so the outcome no longer depends on which DLL me3 mapped first.
                    // hudhook tolerates being installed before the swapchain exists; it hooks
                    // `Present` and waits for the game to call it.
                    let owned =
                        er_build_watermark_core::install_if_owner(module_base, watermark_log);
                    watermark_log(format_args!(
                        "overlay claim -> owner={owned} (false means another module already owns \
                         it and is expected to carry the rows itself)"
                    ));
                });
            if spawned.is_err() {
                watermark_log(format_args!("could not spawn the install thread"));
            }
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_build_watermark_host_stub() -> i32 {
    1
}

// Same reason as every other overlay module: if this shell wins the context, guests have to be
// able to find it by name.
#[cfg(windows)]
er_build_watermark_core::export_overlay_host!();
