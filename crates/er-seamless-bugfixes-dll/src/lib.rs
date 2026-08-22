//! Crash guards for faults only Seamless Co-op's networking mode can reach.
//!
//! Seamless Co-op puts the session into states vanilla Elden Ring reaches rarely or never, and
//! vanilla code paths written against the narrower states fault there. This DLL intercepts those
//! specific functions and returns the value the function itself already returns for the case it
//! failed to handle. It patches nothing in `ersc.dll` and adds no behaviour of its own.
//!
//! Every guard lives in [`guards::REGISTRY`]; see [`guards`] for the rules one has to satisfy. The
//! first is [`guards::null_special_effect`], a null `SpecialEffect` container reached through
//! `CS::SummonBuddyManager::Update`'s networked spirit-summon loop.
//!
//! # Version pinning
//!
//! Guard addresses are 1.16.2 RVAs. Each is re-checked against the live image's bytes before its
//! hook is installed, so a different build disarms that guard and says so in the log rather than
//! detouring whatever now occupies the address.
//!
//! # Why a private MinHook instance, and why that is safe here
//!
//! The repo's usual rule is to route a companion DLL's detours through the product's
//! `er_effects_union_register` export, so a single MinHook instance owns every shared prologue.
//! That is not available to these guards: the union's handler type is
//! `fn(usize, usize, usize, usize) -> usize`, and `er-hook` states plainly that it is "not for
//! float-arg or >4-stack-arg targets". `CS::SpecialEffect::Apply` takes eight arguments, four of
//! them on the stack. Its union dispatcher would forward only the register four and drop the rest,
//! which corrupts the call rather than protecting it -- worse than the crash being guarded.
//!
//! So both guards use this DLL's own MinHook instance, which is safe because no er-effects DLL
//! hooks either address (searched repo-wide), and because the install-time prologue check is itself
//! the collision detector: a second MinHook instance that got there first would have replaced the
//! entry with a jump, the byte comparison would fail, and the guard would disarm loudly instead of
//! trampling a trampoline. If the product ever does hook one of these addresses, that check turns
//! the collision into a log line rather than a silent corruption.

pub(crate) mod guards;

use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_game_base::log::{append_line, game_directory_path};

use crate::guards::{Guard, ORIGINAL_UNSET, REGISTRY};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const LOG_FILE_NAME: &str = "er-seamless-bugfixes.log";

static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn Sleep(ms: u32);
}

fn log_message(args: fmt::Arguments<'_>) {
    let path = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(LOG_FILE_NAME);
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    _module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        START.call_once(spawn_install_task);
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_seamless_bugfixes_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

/// Installation runs off the loader thread: it waits on the game module and calls MinHook, neither
/// of which belongs under the loader lock.
#[cfg(windows)]
fn spawn_install_task() {
    let _ = std::thread::Builder::new()
        .name("er-seamless-bugfixes".to_owned())
        .spawn(|| {
            let mut attempts = 0_u64;
            loop {
                match er_game_base::mem::game_module_base() {
                    Ok(base) => {
                        GAME_BASE.store(base, Ordering::SeqCst);
                        install_guards(base);
                        break;
                    }
                    Err(err) => {
                        if attempts == 0 || attempts.is_multiple_of(4096) {
                            log_message(format_args!(
                                "install: waiting for game module base: {err}"
                            ));
                        }
                        attempts = attempts.saturating_add(1);
                        std::thread::yield_now();
                    }
                }
            }
        });
}

/// Confirm the live bytes at `address` still match what this guard's RVA was verified against.
///
/// Fail-closed, and doing double duty: a drifted prologue means either the running image is not the
/// 1.16.2 build these addresses came from, or something else already detoured the entry. Both are
/// reasons not to install, and both produce a log line naming the bytes actually found.
#[cfg(windows)]
fn prologue_matches(guard: &Guard, address: usize) -> bool {
    let mut actual = [0_u8; 32];
    let Some(window) = actual.get_mut(..guard.expected_prologue.len()) else {
        log_message(format_args!(
            "DISARMED {} @0x{address:x}: expected_prologue is {} bytes, longer than the {} the \
             checker can read",
            guard.name,
            guard.expected_prologue.len(),
            actual.len(),
        ));
        return false;
    };
    if !unsafe { er_game_base::mem::read_bytes(address, window) } {
        log_message(format_args!(
            "DISARMED {} @0x{address:x}: prologue unreadable",
            guard.name
        ));
        return false;
    }
    if window != guard.expected_prologue {
        log_message(format_args!(
            "DISARMED {} @0x{address:x}: prologue mismatch (got {window:02x?}, want {:02x?}) -- \
             either this is not the 1.16.2 build these RVAs were verified against, or the entry is \
             already detoured. Not installing.",
            guard.name, guard.expected_prologue,
        ));
        return false;
    }
    true
}

#[cfg(windows)]
fn install_guards(base: usize) {
    use er_hook::{MH_Initialize, MH_STATUS};

    log_message(format_args!(
        "attach: {} guard(s); game base 0x{base:x}. Each guard returns the guarded function's own \
         'nothing here' value on a null container and is otherwise pass-through.",
        REGISTRY.len()
    ));

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!("install: MH_Initialize failed: {status:?}"));
            return;
        }
    }

    let mut armed = 0_usize;
    for guard in REGISTRY {
        let address = base + guard.rva;
        if !prologue_matches(guard, address) {
            continue;
        }
        if install_guard(guard, address) {
            armed += 1;
            log_message(format_args!(
                "ARMED {} @0x{address:x} (rva 0x{:x}): {}",
                guard.name, guard.rva, guard.rationale
            ));
        }
    }

    log_message(format_args!(
        "install complete: {armed}/{} guard(s) armed",
        REGISTRY.len()
    ));

    // A half-armed pair is not a safe state: the query guard alone sends the crashing caller into
    // the apply path that faults on the same field. Say so rather than letting the log read as a
    // partial success.
    if armed != 0 && armed != REGISTRY.len() {
        log_message(format_args!(
            "WARNING: only {armed}/{} guards armed. These guards are designed to act together -- \
             the query guard alone lets the caller proceed into the apply path that faults on the \
             same field. Treat this run as UNGUARDED.",
            REGISTRY.len()
        ));
    }

    if armed > 0 {
        spawn_telemetry_task();
    }
}

#[cfg(windows)]
fn install_guard(guard: &Guard, address: usize) -> bool {
    use er_hook::{MH_EnableHook, MH_STATUS, MhHook};

    let hook = match unsafe {
        MhHook::new(
            address as *mut core::ffi::c_void,
            guard.detour as *mut core::ffi::c_void,
        )
    } {
        Ok(hook) => hook,
        Err(status) => {
            log_message(format_args!(
                "DISARMED {} @0x{address:x}: MhHook::new failed: {status:?}",
                guard.name
            ));
            return false;
        }
    };
    // Publish the trampoline BEFORE enabling. The stub's pass-through path tail-jumps through this
    // slot, and the first call can land the instant the hook goes live.
    guard
        .original
        .store(hook.trampoline() as usize, Ordering::SeqCst);
    match unsafe { MH_EnableHook(address as *mut core::ffi::c_void) } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ENABLED => true,
        status => {
            guard.original.store(ORIGINAL_UNSET, Ordering::SeqCst);
            log_message(format_args!(
                "DISARMED {} @0x{address:x}: MH_EnableHook failed: {status:?}",
                guard.name
            ));
            false
        }
    }
}

/// How often the telemetry task reports. The guards themselves cannot log -- they are naked stubs
/// on a hot path -- so their counters are read from outside instead.
#[cfg(windows)]
const TELEMETRY_POLL_MS: u32 = 5_000;

/// Report each guard's block count whenever it changes.
///
/// This is the semaphore for the whole DLL. A session where every count is still zero never met the
/// bug and is not evidence that the guard works; a nonzero count is a fault the game would
/// otherwise have taken.
#[cfg(windows)]
fn spawn_telemetry_task() {
    let _ = std::thread::Builder::new()
        .name("er-seamless-bugfixes-telemetry".to_owned())
        .spawn(|| {
            let mut reported = vec![0_u64; REGISTRY.len()];
            loop {
                unsafe { Sleep(TELEMETRY_POLL_MS) };
                for (index, guard) in REGISTRY.iter().enumerate() {
                    let blocked = guard.blocked.load(Ordering::Relaxed);
                    if blocked != reported[index] {
                        log_message(format_args!(
                            "BLOCKED {} guard_blocked_total={blocked} (+{}) -- null container \
                             intercepted; the game would have faulted here",
                            guard.name,
                            blocked.saturating_sub(reported[index]),
                        ));
                        reported[index] = blocked;
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_guard_is_documented_and_distinct() {
        for guard in REGISTRY {
            assert!(!guard.name.is_empty());
            assert!(
                !guard.rationale.is_empty(),
                "{}: a guard must say why its return value is correct",
                guard.name
            );
            assert!(guard.rva != 0);
            assert!(
                !guard.expected_prologue.is_empty(),
                "{}: a guard must pin its address by bytes",
                guard.name
            );
        }
        for (index, guard) in REGISTRY.iter().enumerate() {
            for other in &REGISTRY[index + 1..] {
                assert_ne!(
                    guard.rva, other.rva,
                    "two guards on one address would fight over the same prologue"
                );
            }
        }
    }

    #[test]
    fn guards_start_unarmed() {
        for guard in REGISTRY {
            assert_eq!(
                guard.blocked.load(Ordering::Relaxed),
                0,
                "{}: block counter must start at zero so a nonzero value means a real fault",
                guard.name
            );
            assert_eq!(
                guard.original.load(Ordering::Relaxed),
                ORIGINAL_UNSET,
                "{}: the stub tail-jumps through this slot, so it must be unset until a real \
                 trampoline is published",
                guard.name
            );
        }
    }
}
