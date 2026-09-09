//! Three death conveniences, each one rewritten byte of ELDEN RING's own code.
//!
//! Ported from `github chozandrias76/er-convenient-deaths`, which is where the idea and the
//! choice of three options come from. What is not carried over is its addresses: all three of its
//! offsets are dead on both builds this workspace has an image for, and because each patch checks
//! its own expected bytes and skips on mismatch, a stale offset is a silent no-op rather than a
//! crash -- so a port that trusted them would have installed nothing and said nothing. The sites
//! were re-measured from the functions themselves; `src/patches.rs` carries the derivation.
//!
//! # Shape
//!
//! A `[[natives]]` entry, enabled by presence, with no dependency on the product DLL. It installs
//! no detour and spawns no recurring task: it waits for the game module, writes at most three
//! bytes, logs what happened, and its thread exits. Nothing here runs again for the rest of the
//! session.
//!
//! # What the log is worth
//!
//! `PATCHED` means the byte was written and read back as the intended value. That is a weaker
//! claim than a hook's call counter: the instruction a patch redirects around never runs, so
//! nothing can count it. Evidence that a patch *did* anything has to come from playing -- dying
//! and still having your runes. What the log can prove is the negative, and it does so loudly:
//! `REFUSED` (the containing function has no mapping on this build) and `DISARMED` (the address
//! resolved, but the bytes there are not the measured ones) both mean the game is untouched.

// The install path is `cfg(windows)`: on the host there is no game to patch, so the registry
// rows and the logging they feed have no caller. That is a cfg artifact, not dead code -- and
// without this the crate's own host tests cannot build, let alone run.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

pub(crate) mod config;
pub(crate) mod patches;

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use er_game_base::log::{append_line, game_directory_path};

use crate::patches::{Patch, REGISTRY};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const LOG_FILE_NAME: &str = "er-convenient-deaths.log";

static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn log_message(args: fmt::Arguments<'_>) {
    let path = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(LOG_FILE_NAME);
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}

/// What happened to one patch.
///
/// Four outcomes, not two. Collapsing "the player did not ask for this" into the same bucket as
/// "the bytes were wrong" would make a config typo read exactly like a game update.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The byte was written and read back as intended.
    Patched,
    /// The config did not switch this one on. The game is untouched.
    NotRequested,
    /// No mapping for the containing function on the running build. Nothing read, nothing written.
    Refused,
    /// The address resolved but the bytes there are not the measured ones. Nothing written.
    Disarmed,
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
        // One sink for this DLL's address lines: every cdylib links its own copy of
        // er-game-base, so without this a refused address is silent here. A panic in a cdylib
        // inside the game is otherwise anonymous -- what survives is a 0xe06d7363 record naming
        // the module and nothing else.
        er_game_base::panic_report::report_panics_to("er-convenient-deaths", log_message);
        er_hook::set_hook_logger(log_message);
        let path = windows_runtime::module_path(module);
        START.call_once(move || spawn_install_task(path));
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_convenient_deaths_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

#[cfg(windows)]
mod windows_runtime {
    use std::path::PathBuf;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleFileNameW(module: *mut core::ffi::c_void, filename: *mut u16, size: u32)
        -> u32;
    }

    /// Full path of this DLL, which is what the config sits beside.
    ///
    /// `None` rather than a guess: a config read from the wrong directory would silently apply
    /// somebody else's settings, and the install log says so instead.
    pub(super) fn module_path(module: *mut core::ffi::c_void) -> Option<PathBuf> {
        let mut buffer = [0_u16; 1024];
        // SAFETY: `buffer` is a live array of `buffer.len()` `u16`s, which is exactly the
        // capacity being declared to the call.
        let length = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) }
            as usize;
        if length == 0 || length >= buffer.len() {
            return None;
        }
        String::from_utf16(&buffer[..length])
            .ok()
            .map(PathBuf::from)
    }
}

/// The config path for a loaded module: same name, `.toml` extension.
fn config_path_for_module(module_path: &Path) -> Option<PathBuf> {
    let stem = module_path.file_stem()?;
    Some(
        module_path
            .with_file_name(stem)
            .with_extension(config::CONFIG_EXTENSION),
    )
}

/// Every patch's key and effect, in registry order, for the file written on first run.
fn documented_keys() -> Vec<(&'static str, &'static str)> {
    REGISTRY
        .iter()
        .map(|patch| (patch.config_key, patch.effect))
        .collect()
}

/// Read the config, writing the default file first if there is none.
fn load_config(path: &Path) -> config::Config {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            log_message(format_args!("config: read '{}'", path.display()));
            config::parse(&contents)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let written = config::boilerplate(&documented_keys());
            match std::fs::write(path, &written) {
                Ok(()) => log_message(format_args!(
                    "config: none found, wrote the default to '{}' with every option OFF -- edit \
                     it and restart to enable one",
                    path.display()
                )),
                Err(error) => log_message(format_args!(
                    "config: none found and '{}' could not be written ({error}); every option \
                     stays off",
                    path.display()
                )),
            }
            config::parse(&written)
        }
        Err(error) => {
            log_message(format_args!(
                "config: '{}' is unreadable ({error}); every option stays off",
                path.display()
            ));
            config::Config::default()
        }
    }
}

/// Installation runs off the loader thread: it waits on the game module, which does not belong
/// under the loader lock.
#[cfg(windows)]
fn spawn_install_task(module_path: Option<PathBuf>) {
    let _ = std::thread::Builder::new()
        .name("er-convenient-deaths".to_owned())
        .spawn(move || {
            let Some(config_path) = module_path.as_deref().and_then(config_path_for_module) else {
                log_message(format_args!(
                    "install: this DLL's own path is unknown, so the config beside it cannot be \
                     found; nothing patched"
                ));
                return;
            };
            let config = load_config(&config_path);
            if config.enabled_count() == 0 {
                log_message(format_args!(
                    "install: no option is enabled in '{}'; the game is untouched",
                    config_path.display()
                ));
                return;
            }
            let mut attempts = 0_u64;
            // Bounded on purpose: an unbounded `loop { yield_now() }` in two other shells starved
            // the wineserver and hung a whole boot. See er_game_base::wait.
            let found =
                er_game_base::wait::poll_until(|| match er_game_base::mem::game_module_base() {
                    Ok(base) => Some(base),
                    Err(error) => {
                        if attempts == 0 || attempts.is_multiple_of(4096) {
                            log_message(format_args!(
                                "install: waiting for game module base: {error}"
                            ));
                        }
                        attempts = attempts.saturating_add(1);
                        None
                    }
                });
            let Some(base) = found else {
                log_message(format_args!(
                    "install: no game module base; nothing patched"
                ));
                return;
            };
            install_patches(base, &config);
        });
}

#[cfg(windows)]
fn install_patches(base: usize, config: &config::Config) {
    let mut patched = 0_usize;
    let mut skipped = 0_usize;
    let mut refused = 0_usize;
    let mut disarmed = 0_usize;
    for patch in REGISTRY {
        match apply_patch(patch, base, config) {
            Outcome::Patched => patched += 1,
            Outcome::NotRequested => skipped += 1,
            Outcome::Refused => refused += 1,
            Outcome::Disarmed => disarmed += 1,
        }
    }
    log_message(format_args!(
        "install complete: {patched}/{} PATCHED, {skipped} not requested, {refused} REFUSED (no \
         address on the running build), {disarmed} DISARMED (address resolved, bytes there are \
         not the measured ones). {}",
        REGISTRY.len(),
        er_game_base::game_build::describe_build()
    ));
}

/// Resolve the window, verify it, write the one byte, and read it back.
#[cfg(windows)]
fn apply_patch(patch: &Patch, base: usize, config: &config::Config) -> Outcome {
    if !config.is_enabled(patch.config_key) {
        log_message(format_args!(
            "skipping {} -- '{} = true' is not set",
            patch.name, patch.config_key
        ));
        return Outcome::NotRequested;
    }
    // `resolve_call_site_rva`, not `resolve_detour_address`. The detour resolver answers "may
    // MinHook overwrite five bytes at this function entry", and this is not that question: this
    // address is a single byte inside a function, read and then written. Resolving the enclosing
    // function and adding the offset afterwards is what that helper exists for -- the offset
    // never enters an address table, so nothing can read it as a licence to detour a
    // mid-function address.
    let Some(window) = er_game_base::game_build::resolve_call_site_rva(
        patch.function_rva,
        patch.offset_in_function,
        patch.name,
    )
    .map(|rva| base + rva) else {
        log_message(format_args!(
            "REFUSED {} (1.16.2 window rva 0x{:x} = fn 0x{:x} + 0x{:x}): no mapping for the \
             CONTAINING function on the running build -- {}. Nothing read, nothing written. This \
             is a missing mapping, not a stale signature.",
            patch.name,
            patch.rva(),
            patch.function_rva,
            patch.offset_in_function,
            er_game_base::game_build::describe_build()
        ));
        return Outcome::Refused;
    };
    if !code_window_matches(patch.name, window, patch.expected_window) {
        return Outcome::Disarmed;
    }
    let target = patch.target(window);
    let Some(replaced) = patch.replaced() else {
        log_message(format_args!(
            "DISARMED {} @0x{target:x}: offset {} is outside its own {}-byte window",
            patch.name,
            patch.offset,
            patch.expected_window.len(),
        ));
        return Outcome::Disarmed;
    };
    // SAFETY: `target` is inside the game image. `patch.target(window)` is an offset into the
    // window `code_window_matches` just verified byte-for-byte, at the address the resolver gave
    // for this build, so it is mapped, executable, and holds the instruction this patch was
    // measured against. Both a refusal and a mismatch have already returned above.
    if !unsafe { er_hook::write_code_byte(target, patch.replacement) } {
        log_message(format_args!(
            "DISARMED {} @0x{target:x}: VirtualProtect refused the write",
            patch.name
        ));
        return Outcome::Disarmed;
    }
    // A successful write is not proof the byte landed -- another mod can own the same address --
    // so the value is read back before anything claims to be applied.
    let mut readback = [0_u8; 1];
    // SAFETY: same one verified byte that was just written, read through the same primitive.
    if !unsafe { er_game_base::mem::read_bytes(target, &mut readback) } {
        log_message(format_args!(
            "DISARMED {} @0x{target:x}: the byte was written but could not be read back, so \
             nothing here can say what is at that address",
            patch.name
        ));
        return Outcome::Disarmed;
    }
    if readback[0] != patch.replacement {
        log_message(format_args!(
            "DISARMED {} @0x{target:x}: wrote 0x{:02x} over 0x{replaced:02x} but read back \
             0x{:02x} -- something else owns this byte",
            patch.name, patch.replacement, readback[0],
        ));
        return Outcome::Disarmed;
    }
    patch.applied.store(true, Ordering::SeqCst);
    log_message(format_args!(
        "PATCHED {name} @0x{target:x}: 0x{replaced:02x} -> 0x{replacement:02x}. Effect: {effect}. \
         Sound because {rationale}.",
        name = patch.name,
        replacement = patch.replacement,
        effect = patch.effect,
        rationale = patch.rationale,
    ));
    Outcome::Patched
}

/// Read the window and compare it against the bytes this patch was measured against.
#[cfg(windows)]
fn code_window_matches(label: &str, address: usize, expected: &[u8]) -> bool {
    let mut actual = [0_u8; 32];
    let Some(window) = actual.get_mut(..expected.len()) else {
        log_message(format_args!(
            "DISARMED {label} @0x{address:x}: expected window is {} bytes, longer than the {} the \
             checker can read",
            expected.len(),
            actual.len(),
        ));
        return false;
    };
    // SAFETY: `address` came from the address resolver for the running build, so it names bytes
    // inside the game image; `read_bytes` reports rather than faults if it does not.
    if !unsafe { er_game_base::mem::read_bytes(address, window) } {
        log_message(format_args!(
            "DISARMED {label} @0x{address:x}: window unreadable"
        ));
        return false;
    }
    if window != expected {
        log_message(format_args!(
            "DISARMED {label} @0x{address:x}: byte mismatch (got {window:02x?}, want \
             {expected:02x?}). This address was already resolved for the running build, so the \
             likeliest cause is that another mod rewrote these bytes; the other is that the \
             mapping is wrong. Not patching."
        ));
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_sits_beside_the_dll_under_the_dll_name() {
        let path = config_path_for_module(Path::new("/mods/er_convenient_deaths.dll"))
            .expect("a path with a stem yields a config path");
        assert_eq!(path, Path::new("/mods/er_convenient_deaths.toml"));
    }

    /// A directory has no file stem, and guessing one would read somebody else's config.
    #[test]
    fn a_path_with_no_stem_yields_no_config_path() {
        assert_eq!(config_path_for_module(Path::new("/")), None);
    }

    /// The file written on first run has to document every patch, or the only way to find an
    /// option is to read this source.
    #[test]
    fn every_registry_key_reaches_the_default_config() {
        let written = config::boilerplate(&documented_keys());
        for patch in REGISTRY {
            assert!(
                written.contains(patch.config_key),
                "{} is not named in the default config",
                patch.name
            );
        }
        assert_eq!(
            config::parse(&written).enabled_count(),
            0,
            "the default config must enable nothing"
        );
    }

    /// Every key has to be reachable by the parser, which is a different claim from appearing in
    /// the text: a key with a space or an `=` in it would be written and never matched.
    #[test]
    fn every_registry_key_round_trips_through_the_parser() {
        for patch in REGISTRY {
            let config = config::parse(&format!("{} = true\n", patch.config_key));
            assert!(
                config.is_enabled(patch.config_key),
                "{} cannot be switched on by its own key",
                patch.name
            );
        }
    }
}
