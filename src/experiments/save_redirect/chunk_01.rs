use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
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
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
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

// ===========================================================================
// SAVE-SOURCE OVERRIDE (no-default-fallback, env-mandated)
// ===========================================================================
//
// USER HARD CONSTRAINT (save-override-no-default-fallback-mandatory-env-2026-06-23):
// while the DLL is loaded it MUST NOT assume / read the default user save directory
// (%APPDATA%/EldenRing/<SteamID64>/ER0000.sl2). There is NO escape hatch back to the
// default dir. The ONLY exemption is a pure telemetry/observe-only mode that loads
// nothing. In every other case the save source is MANDATORY and supplied via env
// `ER_EFFECTS_SAVE_FILE` (an absolute path to the save file the game should open);
// if it is unset/blank/not a readable real save the process ABORTS early at DLL init,
// before the game opens any save -- never a silent fallback.
//
// Mechanism: a scoped MinHook on the Win32 `CreateFileW` (and `CopyFileW`) chokepoint
// through which the game opens EVERY save artifact (verified RE: vanilla `.sl2`,
// Seamless `.co2`, `.bak`, all funnel `MicrosoftDiskFileOperator::OpenFile` ->
// `CreateFileW`; reads/writes reuse the returned HANDLE so redirecting the open covers
// both). The hook rewrites only the DIRECTORY portion of paths that match the save
// signature (a `\EldenRing\` segment + a save basename), keeping the game's chosen
// basename, so `.sl2`/`.co2`/`.bak` reroute together and vanilla + Seamless both work.
// Non-save opens pass through unchanged. Stable Win32 ABI; no fixed-offset code poke;
// mod-compatible (ERSC does not replace this open). See target/save-io-re-findings.md.

/// Minimum plausible size (bytes) of a real ER0000.sl2/.co2: the fixed-slot BND4 container
/// is ~28 MB even with empty slots, so anything under 1 MB is missing/truncated/garbage.
pub(crate) const SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES: u64 = 0x10_0000;

/// Telemetry/observe-only exemption: env `ER_EFFECTS_TELEMETRY_ONLY=1` OR GAME_DIR file
/// `er-effects-telemetry-only.txt`. The SOLE case the DLL may run without an env-provided
/// save source, because it loads no character (pure observation).
pub(crate) fn save_override_telemetry_only() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TELEMETRY_ONLY").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-telemetry-only.txt")
        .exists()
}

/// Save-IO TRACE gate (ER_EFFECTS_SAVE_TRACE=1 / er-effects-save-trace.txt). When set, install the
/// save-redirect hooks for their DIAGNOSTICS ONLY (CreateFileW + NtCreateFile path logging) even with
/// NO redirect dir set -- so we can trace how the WORKING vanilla case (a char-present save in the
/// real appdata, no redirect) opens ER0000.sl2. No redirect, no abort; pure observation.
pub(crate) fn save_trace_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_SAVE_TRACE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-save-trace.txt")
            .exists()
}

static OBSERVED_ACTIVE_STEAM_ID64: AtomicU64 = AtomicU64::new(0);

fn observe_steam_id64_from_save_path(path: &[u16]) {
    const ELDENRING: &[u16] = &[
        b'e' as u16,
        b'l' as u16,
        b'd' as u16,
        b'e' as u16,
        b'n' as u16,
        b'r' as u16,
        b'i' as u16,
        b'n' as u16,
        b'g' as u16,
    ];
    let Some(idx) = wide_find_ci_ascii(path, ELDENRING) else {
        return;
    };
    let mut pos = idx + ELDENRING.len();
    while matches!(path.get(pos), Some(c) if *c == b'\\' as u16 || *c == b'/' as u16) {
        pos += 1;
    }
    let start = pos;
    let mut steam_id = 0u64;
    while let Some(&c) = path.get(pos) {
        if !(b'0' as u16..=b'9' as u16).contains(&c) {
            break;
        }
        steam_id = steam_id
            .saturating_mul(10)
            .saturating_add((c - b'0' as u16) as u64);
        pos += 1;
    }
    let digits = pos.saturating_sub(start);
    if (16..=20).contains(&digits) && steam_id != 0 {
        OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
    }
}

/// Read the active signed-in account SteamID64 as observed from the native save path builder's output.
/// The direct native getter exists, but calling it too early can terminate under Arxan/me3; the path
/// builder has already done the native call safely by the time a save path is visible to our hooks.
pub(crate) unsafe fn active_steam_id64(_base: usize) -> Option<u64> {
    let observed = OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst);
    (observed != 0).then_some(observed)
}

fn save_normalize_hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes.iter().copied() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn log_save_steam_id_locations(bytes: &[u8], target: u64, source: &str) {
    match er_save_loader::bnd4::steam_id_locations(bytes) {
        Ok(locations) => {
            let mismatch_count = locations
                .iter()
                .filter(|location| location.value != target)
                .count();
            append_autoload_debug(format_args!(
                "save-steamid-normalize: prewrite source={source} target={target} locations={} mismatches={mismatch_count}",
                locations.len()
            ));
            for location in locations
                .iter()
                .filter(|location| location.value != target)
                .take(16)
            {
                append_autoload_debug(format_args!(
                    "save-steamid-normalize: mismatch source={source} entry={} body_off=0x{:x} file_off=0x{:x} current={} target={target}",
                    location.entry_name, location.body_offset, location.file_offset, location.value
                ));
            }
        }
        Err(err) => append_autoload_debug(format_args!(
            "save-steamid-normalize: prewrite inspect failed source={source}: {err:?}"
        )),
    }
}

pub(crate) fn normalize_save_bytes_to_active_steam_id(
    base: usize,
    bytes: &mut [u8],
    source: &str,
) -> Option<er_save_loader::bnd4::SteamIdNormalizeReport> {
    let Some(steam_id) = (unsafe { active_steam_id64(base) }) else {
        append_autoload_debug(format_args!(
            "save-steamid-normalize: skipped source={source} -- active SteamID64 unavailable"
        ));
        return None;
    };
    log_save_steam_id_locations(bytes, steam_id, source);
    match er_save_loader::bnd4::normalize_steam_id_in_place(bytes, steam_id) {
        Ok(report) => {
            append_autoload_debug(format_args!(
                "save-steamid-normalize: source={source} steam_id={steam_id} char_seen={} char_patched={} user_data10_seen={} user_data10_patched={} md5_rewritten={}",
                report.character_slots_seen,
                report.character_slots_patched,
                report.user_data10_seen,
                report.user_data10_patched,
                report.md5_rewritten
            ));
            Some(report)
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "save-steamid-normalize: failed source={source}: {err:?}"
            ));
            None
        }
    }
}

pub(crate) fn normalize_env_save_file_to_active_steam_id_once(base: usize, reason: &str) {
    if SAVE_STEAM_ID_ENV_NORMALIZE_DONE.load(Ordering::SeqCst) != 0
        || OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst) == 0
    {
        return;
    }
    let Some(path) = configured_save_file() else {
        append_autoload_debug(format_args!(
            "save-steamid-normalize: no configured save file for one-shot disk normalize reason={reason}"
        ));
        return;
    };
    let Ok(mut bytes) = fs::read(&path) else {
        append_autoload_debug(format_args!(
            "save-steamid-normalize: failed to read configured save file for one-shot disk normalize reason={reason} path='{}'",
            path.display()
        ));
        return;
    };
    let before = save_normalize_hash_bytes(&bytes);
    let Some(report) = normalize_save_bytes_to_active_steam_id(base, &mut bytes, reason) else {
        return;
    };
    if !report.changed() {
        SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
        return;
    }
    match fs::write(&path, &bytes) {
        Ok(()) => {
            let after = save_normalize_hash_bytes(&bytes);
            SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-steamid-normalize: wrote normalized env save path='{}' reason={reason} before=0x{before:016x} after=0x{after:016x}",
                path.display()
            ));
        }
        Err(err) => {
            SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-steamid-normalize: FAILED to write normalized env save path='{}' reason={reason}: {err}",
                path.display()
            ));
        }
    }
}

/// Redirect directory (UTF-16, NUL-free, no trailing separator) computed from the parent of
/// `ER_EFFECTS_SAVE_FILE`. Set once at init, BEFORE the CreateFileW hook is armed.
static SAVE_REDIRECT_DIR_W: OnceLock<Vec<u16>> = OnceLock::new();
/// Original CreateFileW / CopyFileW (MinHook trampolines). 0 = not hooked.
static SAVE_REDIRECT_ORIG_CREATEFILEW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_ORIG_COPYFILEW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Save-existence-check redirects: the game stats/enumerates the save file BEFORE opening it; if
/// these hit the (wiped) default dir the game concludes "no save" and never CreateFileW's it.
static SAVE_REDIRECT_ORIG_GETATTRW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_ORIG_GETATTREXW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_ORIG_FINDFIRSTW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// PRIMARY redirect: the save-dir builder (FUN_140e0e680) calls SHGetFolderPathW(CSIDL_APPDATA) to
/// get %APPDATA%, then formats `%APPDATA%/EldenRing/<steamid>/`. Returning OUR staged root here makes
/// the game build AND open the full save path under our tree NATIVELY (Wine does case-insensitive
/// resolution), so the character is read without depending on intercepting each handle-relative open.
static SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_SHGFP_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// One-shot redirect latch (user design 2026-06-23): the gold is provided via the Z: staged dir for
/// the FIRST load (reading from Z: works), but writing to Z: fails (Wine free-space) AND would mutate
/// the user's save. So once the gold profile is loaded (profile_slot_active != 0), we STOP redirecting
/// -- SHGetFolderPathW reverts to the real %APPDATA% so the system-save WRITE and all subsequent
/// load/save paths land on the proper default C: dir (write works, gold never touched).
pub(crate) static SAVE_FIRST_LOAD_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// ntdll NtCreateFile diagnostic: the boot save read happens BELOW Win32 (no CreateFileW/
/// GetFileAttributesW/FindFirstFileW hit the save), so hook the ntdll chokepoint to SEE the actual
/// open of ER0000.sl2 -- its NT path form and whether it is relative to a RootDirectory handle.
static SAVE_REDIRECT_ORIG_NTCREATEFILE: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_NTCREATE_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
const SAVE_NTCREATE_DIAG_MAX: usize = 120;
/// THE corruption fix (corrupted-save-re-findings): the save commit prechecks free space via
/// GetDiskFreeSpaceExW(saveDir), which on the Wine Z:->/home drive mapping returns bogus/ZERO free
/// space -> `free < needed` -> the write aborts BEFORE any byte ("Failed to save game / corrupted").
/// We hook it to report ample free space for the save dir so the game's OWN save flow writes our
/// staged save (no hardcoded paths, no Steam Cloud).
static SAVE_REDIRECT_ORIG_GETDISKFREEW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_DISKFREE_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// The game doesn't call kernel32!GetDiskFreeSpaceExW from our hook (no fire) -- under Wine all
/// free-space queries funnel to ntdll!NtQueryVolumeInformationFile. Override the AVAILABLE allocation
/// units for FileFsSizeInformation(3)/FileFsFullSizeInformation(7) so the save-commit free-space
/// precheck sees ample space regardless of the bogus Z:-drive report. THE corruption fix, robust.
static SAVE_REDIRECT_ORIG_NTQUERYVOLINFO: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_VOLINFO_LOGGED: AtomicUsize = AtomicUsize::new(0);
static SAVE_REDIRECT_INSTALL_ONCE: Once = Once::new();
static SAVE_STEAM_ID_ENV_NORMALIZE_DONE: AtomicUsize = AtomicUsize::new(0);
/// Count of save-path opens we have redirected, logged for the first few so a probe can CONFIRM the
/// game actually opened our staged save through the redirect (not the default dir). Capped so a
/// busy IO loop cannot spam the debug log.
static SAVE_REDIRECT_HITS: AtomicUsize = AtomicUsize::new(0);
const SAVE_REDIRECT_LOG_MAX: usize = 8;
/// Diagnostic: total CreateFileW calls our detour observed (proves the hook is live at all under
/// Wine's kernel32->kernelbase forwarding), and a bounded log of save-LIKE paths so we can see the
/// exact path form the game opens the save with (to fix the filter or confirm a missed hook).
static SAVE_CREATEFILEW_CALLS: AtomicUsize = AtomicUsize::new(0);
static SAVE_CREATEFILEW_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
const SAVE_CREATEFILEW_DIAG_MAX: usize = 200;
/// Sparse-sampling counter for the save-LIKE CreateFileW diag line (the `save_like` opens churn
/// thousands of identical lines per run). Logs the first 8 hits then only at power-of-two intervals
/// (16/32/64/...) -- same rate-limit pattern as `now_loading_helper_update_hook` -- so the diagnostic
/// keeps its early window and a sparse tail without flooding the debug log.
static SAVE_CREATEFILEW_DIAG_HITS: AtomicUsize = AtomicUsize::new(0);
/// DEDICATED budget for save-FILE queries (paths ending .sl2 / .co2 or containing ER0000): the shared
/// CreateFileW/existence-check diag cap above is exhausted by early-boot `eldenring\` dir churn
/// (GraphicsConfig.xml etc.) BEFORE the actual save read, hiding whether/with-what-steamid the game
/// ever queries ER0000.sl2. This separate counter guarantees those queries are always logged. Reveals
/// the exact `EldenRing\<steamid>\ER0000.sl2` path the game builds (steamid match vs the staged 766).
static SAVE_SL2_QUERY_LOGGED: AtomicUsize = AtomicUsize::new(0);
const SAVE_SL2_QUERY_MAX: usize = 40;
/// Log EVERY CreateFileW path for the first N calls (the whole early-boot save-detection window), so
/// we can see exactly what the game opens after our staged EldenRing\ dir (why it never reads the
/// save). Beyond this, only save-LIKE paths are logged.
const SAVE_CREATEFILEW_DIAG_ALL_BELOW: usize = 120;

/// Frames of "profile summary present but ZERO active slots" tolerated before the save-load watchdog
/// aborts. ~15s at 60fps -- long enough to ignore the boot transient before the summary is parsed,
/// short enough to fast-fail well under the runtime cap instead of stalling on the privacy policy.
pub(crate) static SAVE_WATCHDOG_ZERO_FRAMES: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SAVE_WATCHDOG_ZERO_BUDGET: usize = 900;

/// Convert a configured path root to the Wine drive form the in-process `CreateFileW` accepts.
/// Unix absolute paths become `Z:\...`; already-Windows/Wine paths like `Z:\...` or `C:\...` are
/// preserved. Backslash separators, no trailing separator. Returns a wide string.
fn path_root_to_wine_wide(root: &std::path::Path) -> Vec<u16> {
    // to_string_lossy: building a path string, not decoding game memory (the from_utf8_lossy ban
    // targets in-process telemetry; OsStr->String here is fine).
    let win: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let has_drive_prefix = win.as_bytes().get(1).copied() == Some(b':');
    let mut out: Vec<u16> = if has_drive_prefix {
        win.encode_utf16().collect()
    } else {
        "Z:".encode_utf16().chain(win.encode_utf16()).collect()
    };
    while matches!(out.last(), Some(&c) if c == b'\\' as u16) {
        out.pop();
    }
    out
}

/// Resolve configured save file -> the staged save ROOT (the ancestor directory that CONTAINS the
/// `EldenRing` folder) in Wine `Z:\...` wide form, or None if config/env is unset/blank/not a readable
/// plausibly-sized save / not staged under an `EldenRing` directory component. The redirect rewrites
/// the game's `...\Roaming\EldenRing\<rest>` to `<root>\EldenRing\<rest>`, so the staged save MUST
/// live at `<root>/EldenRing/<steamid>/ER0000.sl2`.
fn env_save_file_path() -> Option<PathBuf> {
    configured_save_file()
}

fn steam_id64_from_env_save_file_path(path: &Path) -> Option<u64> {
    let mut after_elden_ring = false;
    for comp in path.components() {
        let text = comp.as_os_str().to_string_lossy();
        if after_elden_ring {
            if text.len() >= 16
                && text.len() <= 20
                && text.as_bytes().iter().all(u8::is_ascii_digit)
            {
                return text.parse::<u64>().ok().filter(|value| *value != 0);
            }
            after_elden_ring = false;
        }
        if text.eq_ignore_ascii_case("EldenRing") {
            after_elden_ring = true;
        }
    }
    None
}

fn normalize_env_save_file_to_known_steam_id(path: &Path, steam_id: u64, reason: &str) {
    let Ok(mut bytes) = fs::read(path) else {
        append_autoload_debug(format_args!(
            "save-steamid-normalize: failed to read env file for early normalize reason={reason} path='{}'",
            path.display()
        ));
        return;
    };
    let before = save_normalize_hash_bytes(&bytes);
    log_save_steam_id_locations(&bytes, steam_id, reason);
    match er_save_loader::bnd4::normalize_steam_id_in_place(&mut bytes, steam_id) {
        Ok(report) => {
            append_autoload_debug(format_args!(
                "save-steamid-normalize: source={reason} steam_id={steam_id} char_seen={} char_patched={} user_data10_seen={} user_data10_patched={} md5_rewritten={}",
                report.character_slots_seen,
                report.character_slots_patched,
                report.user_data10_seen,
                report.user_data10_patched,
                report.md5_rewritten
            ));
            if !report.changed() {
                return;
            }
            match fs::write(path, &bytes) {
                Ok(()) => {
                    let after = save_normalize_hash_bytes(&bytes);
                    append_autoload_debug(format_args!(
                        "save-steamid-normalize: wrote normalized env save path='{}' reason={reason} before=0x{before:016x} after=0x{after:016x}",
                        path.display()
                    ));
                }
                Err(err) => append_autoload_debug(format_args!(
                    "save-steamid-normalize: FAILED to write normalized env save path='{}' reason={reason}: {err}",
                    path.display()
                )),
            }
        }
        Err(err) => append_autoload_debug(format_args!(
            "save-steamid-normalize: failed source={reason}: {err:?}"
        )),
    }
}

fn save_override_redirect_root_w() -> Option<Vec<u16>> {
    let path = env_save_file_path()?;
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() < SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES {
        return None;
    }
    let mut root = PathBuf::new();
    let mut found = false;
    for comp in path.components() {
        if comp
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("EldenRing")
        {
            found = true;
            break;
        }
        root.push(comp);
    }
    if !found {
        return None;
    }
    Some(path_root_to_wine_wide(&root))
}

/// Outcome of `enforce_save_override_or_abort`. The abort path does not return.
pub(crate) enum SaveOverrideMode {
    /// Pure telemetry/observe-only: no save source required, no redirect installed.
    TelemetryOnly,
    /// A valid env save source was resolved; the redirect hook should be installed.
    Redirect,
}

/// Called EARLY in `DllMain` (before any save IO). Enforces the no-default-fallback rule:
/// unless telemetry-only, a valid DLL-adjacent `er-effects.toml` save source (optionally overridden
/// by `ER_EFFECTS_SAVE_FILE`) MUST be present, else the process is aborted immediately. On success it
/// stashes the redirect directory for the CreateFileW hook.
/// NEVER returns on the fail-closed path.
pub(crate) fn enforce_save_override_or_abort() -> SaveOverrideMode {
    if save_override_telemetry_only() {
        append_autoload_debug(format_args!(
            "save-override: TELEMETRY-ONLY mode -- save source not enforced (loads nothing; no default-dir read for a character)"
        ));
        return SaveOverrideMode::TelemetryOnly;
    }
    match save_override_redirect_root_w() {
        Some(root_w) => {
            if let Some(path) = env_save_file_path() {
                if let Some(steam_id) = steam_id64_from_env_save_file_path(&path) {
                    OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
                    normalize_env_save_file_to_known_steam_id(
                        &path,
                        steam_id,
                        "early-enforced-env-save",
                    );
                    SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
                } else {
                    append_autoload_debug(format_args!(
                        "save-steamid-normalize: early env path has no EldenRing/<steamid> segment"
                    ));
                }
            }
            // UTF-8 Lossy: log-only decode of the staged root for probe confirmation.
            let shown = String::from_utf16_lossy(&root_w);
            let _ = SAVE_REDIRECT_DIR_W.set(root_w);
            append_autoload_debug(format_args!(
                "save-override: ENFORCED -- redirecting the whole %APPDATA%\\Roaming\\EldenRing save subtree to staged root '{shown}' (expects <root>\\EldenRing\\<steamid>\\ER0000.sl2)"
            ));
            SaveOverrideMode::Redirect
        }
        None => {
            // FAIL CLOSED. The DLL must never assume the default user save directory.
            append_autoload_debug(format_args!(
                "save-override: FATAL -- configured save file is unset/blank/not a readable save (>= {} bytes) staged under an EldenRing dir, and ER_EFFECTS_TELEMETRY_ONLY is not set. config_error={}. Refusing to assume the default user save directory. ABORTING.",
                SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES,
                runtime_config_error().unwrap_or_else(|| "none".to_owned())
            ));
            eprintln!(
                "er-effects: FATAL -- no valid save source from DLL-adjacent er-effects.toml (or ER_EFFECTS_SAVE_FILE override) and not telemetry-only; refusing to assume the default user save directory. Aborting."
            );
            std::process::abort();
        }
    }
}

/// Length of a NUL-terminated UTF-16 string at `ptr` (excludes the NUL). 0 on null pointer.
unsafe fn wide_len(ptr: *const u16) -> usize {
    if ptr.is_null() {
        return 0;
    }
    let mut len = 0usize;
    // Bounded scan: a path longer than this is not a real Windows path; stop to stay safe.
    const WIDE_SCAN_MAX: usize = 0x8000;
    while len < WIDE_SCAN_MAX {
        if unsafe { *ptr.add(len) } == 0 {
            break;
        }
        len += 1;
    }
    len
}

/// ASCII-lowercase a UTF-16 code unit (leaves non-ASCII untouched).
fn wide_ascii_lower(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

/// True if `hay` contains `needle` (ASCII, case-insensitive). `needle` must be ASCII lowercase.
fn wide_contains_ci_ascii(hay: &[u16], needle: &[u16]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    let last = hay.len() - needle.len();
    (0..=last).any(|start| {
        needle
            .iter()
            .enumerate()
            .all(|(i, &n)| wide_ascii_lower(hay[start + i]) == n)
    })
}

/// First index in `hay` where `needle` occurs (ASCII, case-insensitive). `needle` must be ASCII
/// lowercase. None if absent.
fn wide_find_ci_ascii(hay: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    (0..=last).find(|&start| {
        needle
            .iter()
            .enumerate()
            .all(|(i, &n)| wide_ascii_lower(hay[start + i]) == n)
    })
}

/// True if `hay` ends with `suffix` (ASCII, case-insensitive). `suffix` must be ASCII lowercase.
fn wide_ends_with_ci_ascii(hay: &[u16], suffix: &[u16]) -> bool {
    if suffix.len() > hay.len() {
        return false;
    }
    let start = hay.len() - suffix.len();
    suffix
        .iter()
        .enumerate()
        .all(|(i, &s)| wide_ascii_lower(hay[start + i]) == s)
}

/// Index just after the last path separator in `path` (0 if none) -- the basename start.
fn wide_basename_start(path: &[u16]) -> usize {
    let mut start = 0usize;
    for (i, &c) in path.iter().enumerate() {
        if c == b'\\' as u16 || c == b'/' as u16 {
            start = i + 1;
        }
    }
    start
}

/// If `path` is anywhere under the game's `%APPDATA%\Roaming\EldenRing` save root, return its
/// redirected (NUL-terminated) wide path under our staged EldenRing tree. None = not the save root.
///
/// We redirect the ENTIRE EldenRing-appdata SUBTREE (the `...\Roaming\EldenRing` directory handle and
/// everything under it), not just `*.sl2` files: the game decides "save present?" by ENUMERATING the
/// `EldenRing\` directory handle (Wine NtQueryDirectoryFile), never opening `<steamid>\ER0000.sl2` by
/// path -- so a per-file redirect can't be seen. By rewriting the directory open itself, the
/// handle-relative enumeration lists OUR staged `EldenRing\<steamid>\ER0000.sl2`.
///
/// `SAVE_REDIRECT_DIR_W` holds the staged ROOT that CONTAINS the `EldenRing` folder, in Wine form
/// (`Z:\home\...\save`). The redirect keeps the `EldenRing\<rest>` suffix: game
/// `C:\users\steamuser\AppData\Roaming\EldenRing\<id>\ER0000.sl2` -> `<root>\EldenRing\<id>\ER0000.sl2`.
fn save_redirect_path(path: &[u16]) -> Option<Vec<u16>> {
    let root = SAVE_REDIRECT_DIR_W.get()?;
    const ELDENRING: &[u16] = &[
        b'e' as u16,
        b'l' as u16,
        b'd' as u16,
        b'e' as u16,
        b'n' as u16,
        b'r' as u16,
        b'i' as u16,
        b'n' as u16,
        b'g' as u16,
    ];
    const ROAMING: &[u16] = &[
        b'r' as u16,
        b'o' as u16,
        b'a' as u16,
        b'm' as u16,
        b'i' as u16,
        b'n' as u16,
        b'g' as u16,
    ];
    // Always learn the native/staged `<steamid>` segment from save-like paths; this is the safest
    // current-account oracle because the native save-dir builder already called Steam before the path
    // reached our hook. The redirect decision below is still anchored on `Roaming` to avoid loops.
    observe_steam_id64_from_save_path(path);
    // Anchor on `Roaming` + `EldenRing` so a coincidental "eldenring" elsewhere -- and our already-
    // redirected target (`Z:\...\save\EldenRing\...`, no "Roaming") -- never re-redirects.
    if !wide_contains_ci_ascii(path, ROAMING) {
        return None;
    }
    let idx = wide_find_ci_ascii(path, ELDENRING)?;
    let suffix = &path[idx..]; // "EldenRing\<id>\ER0000.sl2" (or "EldenRing\" for the dir open)
    let mut out = Vec::with_capacity(root.len() + 1 + suffix.len() + 1);
    out.extend_from_slice(root);
    out.push(b'\\' as u16);
    // ASCII-lowercase the suffix: the game opens the save root in MIXED case ("EldenRing\" for the
    // dir handle, "eldenring\graphicsconfig.xml" elsewhere). Our staged tree is on a CASE-SENSITIVE
    // Linux filesystem, so we normalize every case-variant to lowercase and stage the tree lowercase
    // (eldenring/<steamid>/er0000.sl2). The game reads through the returned HANDLE -- it does not care
    // about the redirected filename's case; the Windows-side case-insensitive name compare still
    // matches the enumerated lowercase entries.
    for &c in suffix {
        out.push(wide_ascii_lower(c));
    }
    out.push(0);
    Some(out)
}

type CreateFileWFn =
    unsafe extern "system" fn(*const u16, u32, u32, *const c_void, u32, u32, isize) -> isize;
type CopyFileWFn = unsafe extern "system" fn(*const u16, *const u16, i32) -> i32;

/// CreateFileW detour: redirect save-file opens to the env dir; pass everything else through.
/// Covers BOTH read and write (the returned HANDLE is reused by ReadFile/WriteFile).
unsafe extern "system" fn save_redirect_createfilew_hook(
    lp_file_name: *const u16,
    access: u32,
    share: u32,
    security: *const c_void,
    disposition: u32,
    flags: u32,
    template: isize,
) -> isize {
    let orig = SAVE_REDIRECT_ORIG_CREATEFILEW.load(Ordering::SeqCst);
    let call: CreateFileWFn = unsafe { std::mem::transmute::<usize, CreateFileWFn>(orig) };
    let len = unsafe { wide_len(lp_file_name) };
    let calls = SAVE_CREATEFILEW_CALLS.fetch_add(1, Ordering::SeqCst);
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        // Diagnostic: confirm the hook is live (log the very first call), then log save-LIKE paths
        // (contain "eldenring" or end .sl2/.co2/.bak) so we can see the exact save path form even when
        // the redirect filter does NOT match -- distinguishes "hook never fires" from "filter misses".
        const ELDENRING_SEG: &[u16] = &[
            b'e' as u16,
            b'l' as u16,
            b'd' as u16,
            b'e' as u16,
            b'n' as u16,
            b'r' as u16,
            b'i' as u16,
            b'n' as u16,
            b'g' as u16,
        ];
        const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
        const CO2D: &[u16] = &[b'.' as u16, b'c' as u16, b'o' as u16, b'2' as u16];
        const BAKD: &[u16] = &[b'.' as u16, b'b' as u16, b'a' as u16, b'k' as u16];
        let save_like = wide_contains_ci_ascii(path, ELDENRING_SEG)
            || wide_ends_with_ci_ascii(path, SL2D)
            || wide_ends_with_ci_ascii(path, CO2D)
            || wide_ends_with_ci_ascii(path, BAKD);
        if calls == 0 || save_like {
            // Rate-limit: log the first 8 save-LIKE opens, then only at power-of-two hit counts.
            let hits = SAVE_CREATEFILEW_DIAG_HITS.fetch_add(1, Ordering::SeqCst) + 1;
            if hits <= 8 || hits.is_power_of_two() {
                // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
                let p = String::from_utf16_lossy(path);
                append_autoload_debug(format_args!(
                    "save-override: CreateFileW diag call#{calls} save_like={save_like} diag_hits={hits} '{p}'"
                ));
            }
        }
        let is_save_file =
            wide_ends_with_ci_ascii(path, SL2D) || wide_ends_with_ci_ascii(path, CO2D);
        let redirected_path = save_redirect_path(path);
        if is_save_file {
            if let Ok(base) = game_module_base() {
                normalize_env_save_file_to_active_steam_id_once(base, "createfile-save-open");
            }
        }
        if let Some(redirected) = redirected_path {
            let ret = unsafe {
                call(
                    redirected.as_ptr(),
                    access,
                    share,
                    security,
                    disposition,
                    flags,
                    template,
                )
            };
            let hit = SAVE_REDIRECT_HITS.fetch_add(1, Ordering::SeqCst);
            if hit < SAVE_REDIRECT_LOG_MAX {
                // UTF-8 Lossy: log-only decode of a Windows wide path for probe confirmation.
                let from = String::from_utf16_lossy(path);
                let to_end = redirected
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(redirected.len());
                // UTF-8 Lossy: log-only decode of the redirected wide path.
                let to = String::from_utf16_lossy(&redirected[..to_end]);
                // ret == -1 (INVALID_HANDLE_VALUE) means the redirected path did NOT resolve (Wine
                // path/case miss) -> the game falls back to no-save. ok=true means our file opened.
                let ok = ret != -1;
                append_autoload_debug(format_args!(
                    "save-override: REDIRECT #{hit} access=0x{access:x} disp={disposition} ok={ok} ret=0x{ret:x} '{from}' -> '{to}'"
                ));
            }
            return ret;
        }
    }
    unsafe {
        call(
            lp_file_name,
            access,
            share,
            security,
            disposition,
            flags,
            template,
        )
    }
}

/// CopyFileW detour: redirect either endpoint that is a save artifact (the `.bak` backup routine
/// copies ER0000.sl2 -> ER0000.sl2.bak), so backups follow the save into the env dir and never
/// touch the default user directory.
unsafe extern "system" fn save_redirect_copyfilew_hook(
    existing: *const u16,
    new_file: *const u16,
    fail_if_exists: i32,
) -> i32 {
    let orig = SAVE_REDIRECT_ORIG_COPYFILEW.load(Ordering::SeqCst);
    let call: CopyFileWFn = unsafe { std::mem::transmute::<usize, CopyFileWFn>(orig) };
    let existing_red = {
        let len = unsafe { wide_len(existing) };
        (len != 0)
            .then(|| unsafe { std::slice::from_raw_parts(existing, len) })
            .and_then(save_redirect_path)
    };
    let new_red = {
        let len = unsafe { wide_len(new_file) };
        (len != 0)
            .then(|| unsafe { std::slice::from_raw_parts(new_file, len) })
            .and_then(save_redirect_path)
    };
    let existing_ptr = existing_red.as_ref().map_or(existing, |v| v.as_ptr());
    let new_ptr = new_red.as_ref().map_or(new_file, |v| v.as_ptr());
    unsafe { call(existing_ptr, new_ptr, fail_if_exists) }
}

/// Shared diag + redirect decision for a save-existence-check API taking a wide path arg1. Logs
/// "eldenring"-containing paths (capped, shared budget) so we see the exact existence-check path
/// form, and returns the redirected NUL-terminated path when the save filter matches (else None).
fn save_path_api_redirect(api: &str, path: &[u16]) -> Option<Vec<u16>> {
    const ELDENRING_SEG: &[u16] = &[
        b'e' as u16,
        b'l' as u16,
        b'd' as u16,
        b'e' as u16,
        b'n' as u16,
        b'r' as u16,
        b'i' as u16,
        b'n' as u16,
        b'g' as u16,
    ];
    let redirected = save_redirect_path(path);
    // DEDICATED save-FILE query log (own budget; immune to the early-boot churn that exhausts the
    // shared cap below) -- captures the exact ER0000.sl2 existence/enum path + its <steamid> component.
    const ER0000: &[u16] = &[
        b'e' as u16,
        b'r' as u16,
        b'0' as u16,
        b'0' as u16,
        b'0' as u16,
        b'0' as u16,
    ];
    if wide_contains_ci_ascii(path, ER0000) {
        let d = SAVE_SL2_QUERY_LOGGED.fetch_add(1, Ordering::SeqCst);
        if d < SAVE_SL2_QUERY_MAX {
            // UTF-8 Lossy: log-only decode of the save-file query path for probe diagnosis.
            let p = String::from_utf16_lossy(path);
            let did = if redirected.is_some() {
                "REDIRECT"
            } else {
                "pass"
            };
            append_autoload_debug(format_args!("save-override: {api} SL2-QUERY {did} '{p}'"));
        }
    }
    if wide_contains_ci_ascii(path, ELDENRING_SEG) {
        let d = SAVE_CREATEFILEW_DIAG_LOGGED.load(Ordering::SeqCst);
        if d < SAVE_CREATEFILEW_DIAG_MAX {
            SAVE_CREATEFILEW_DIAG_LOGGED.store(d + 1, Ordering::SeqCst);
            // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
            let p = String::from_utf16_lossy(path);
            let did = if redirected.is_some() {
                "REDIRECT"
            } else {
                "pass"
            };
            append_autoload_debug(format_args!("save-override: {api} diag {did} '{p}'"));
        }
    }
    redirected
}

/// GetFileAttributesW detour: redirect save-path existence checks to the env dir.
unsafe extern "system" fn save_redirect_getattrw_hook(lp_file_name: *const u16) -> u32 {
    let orig = SAVE_REDIRECT_ORIG_GETATTRW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16) -> u32 =
        unsafe { std::mem::transmute::<usize, unsafe extern "system" fn(*const u16) -> u32>(orig) };
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("GetFileAttributesW", path) {
            return unsafe { call(red.as_ptr()) };
        }
    }
    unsafe { call(lp_file_name) }
}

/// GetFileAttributesExW detour: same redirect for the Ex existence check.
unsafe extern "system" fn save_redirect_getattrexw_hook(
    lp_file_name: *const u16,
    info_level: i32,
    info: *mut c_void,
) -> i32 {
    let orig = SAVE_REDIRECT_ORIG_GETATTREXW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32 = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32>(
            orig,
        )
    };
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("GetFileAttributesExW", path) {
            return unsafe { call(red.as_ptr(), info_level, info) };
        }
    }
    unsafe { call(lp_file_name, info_level, info) }
}

/// FindFirstFileW detour: redirect save-path enumeration/existence checks to the env dir.
unsafe extern "system" fn save_redirect_findfirstw_hook(
    lp_file_name: *const u16,
    find_data: *mut c_void,
) -> isize {
    let orig = SAVE_REDIRECT_ORIG_FINDFIRSTW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16, *mut c_void) -> isize = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, *mut c_void) -> isize>(
            orig,
        )
    };
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("FindFirstFileW", path) {
            return unsafe { call(red.as_ptr(), find_data) };
        }
    }
    unsafe { call(lp_file_name, find_data) }
}
