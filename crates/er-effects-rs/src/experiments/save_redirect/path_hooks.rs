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
// SAVE-SOURCE OVERRIDE / DEFAULT-SAVE FALLBACK
// ===========================================================================
//
// Explicit save sources still come from `ER_EFFECTS_SAVE_FILE` or DLL-adjacent
// `er-effects.toml` (`save_file = "..."`). If neither is provided, the product path
// now intentionally falls back to the active Steam user's default save file at
// `%APPDATA%/EldenRing/<SteamID64>/ER0000.sl2`; if that default save does not exist,
// the DLL prompts with the missing-save picker (OK -> choose a save, Cancel -> exit)
// instead of drifting into a no-character menu. Pure telemetry/observe-only mode
// remains the only no-load exemption.
//
// Explicit-source mechanism: a scoped MinHook on the Win32 `CreateFileW` (and `CopyFileW`) chokepoint
// through which the game opens EVERY save artifact (verified RE: vanilla `.sl2`,
// Seamless `.co2`, `.bak`, all funnel `MicrosoftDiskFileOperator::OpenFile` ->
// `CreateFileW`; reads/writes reuse the returned HANDLE so redirecting the open covers
// both). The configured source can be any readable `.sl2`/`.co2` path. Save-file opens
// redirect to that exact file, and directory/existence probes redirect to a private
// staged tree so the native save-discovery flow can still see an `EldenRing/<SteamID>`
// shape internally. Non-save opens pass through unchanged. Stable Win32 ABI; no fixed-
// offset code poke; mod-compatible (ERSC does not replace this open).

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

fn steam_id64_from_wide_save_path(path: &[u16]) -> Option<u64> {
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
    let mut search_from = 0usize;
    while search_from < path.len() {
        let Some(rel_idx) = wide_find_ci_ascii(&path[search_from..], ELDENRING) else {
            break;
        };
        let idx = search_from + rel_idx;
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
            return Some(steam_id);
        }
        search_from = idx + 1;
    }
    None
}

fn observe_steam_id64_from_save_path(path: &[u16]) {
    if let Some(steam_id) = steam_id64_from_wide_save_path(path) {
        OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
        if let Ok(base) = game_module_base() {
            normalize_env_save_file_to_active_steam_id_once(base, "observed-steamid-before-stage");
        }
        ensure_direct_stage_for_steam_id(steam_id);
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

fn path_eq_ignore_ascii_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(
            b.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\'),
        )
}

fn save_file_writeback_allowed(path: &Path) -> bool {
    let Some(default_root) = default_save_root() else {
        return false;
    };
    path.parent()
        .and_then(|steam_dir| steam_dir.parent())
        .is_some_and(|root| path_eq_ignore_ascii_case(root, &default_root))
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
                "save-steamid-normalize: source={reason} steam_id={steam_id} char_seen={} char_patched={} user_data10_seen={} user_data10_patched={} md5_rewritten={} changed={} writeback_allowed={}",
                report.character_slots_seen,
                report.character_slots_patched,
                report.user_data10_seen,
                report.user_data10_patched,
                report.md5_rewritten,
                report.changed(),
                save_file_writeback_allowed(path)
            ));
            if !report.changed() || !save_file_writeback_allowed(path) {
                return;
            }
            match fs::write(path, &bytes) {
                Ok(()) => {
                    let after = save_normalize_hash_bytes(&bytes);
                    append_autoload_debug(format_args!(
                        "save-steamid-normalize: wrote normalized GAME save path='{}' reason={reason} before=0x{before:016x} after=0x{after:016x}",
                        path.display()
                    ));
                }
                Err(err) => append_autoload_debug(format_args!(
                    "save-steamid-normalize: FAILED to write normalized GAME save path='{}' reason={reason}: {err}",
                    path.display()
                )),
            }
        }
        Err(err) => append_autoload_debug(format_args!(
            "save-steamid-normalize: failed source={reason}: {err:?}"
        )),
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
    SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
    if !report.changed() || !save_file_writeback_allowed(&path) {
        append_autoload_debug(format_args!(
            "save-steamid-normalize: one-shot source normalize reason={reason} path='{}' changed={} writeback_allowed={}",
            path.display(),
            report.changed(),
            save_file_writeback_allowed(&path)
        ));
        return;
    }
    match fs::write(&path, &bytes) {
        Ok(()) => {
            let after = save_normalize_hash_bytes(&bytes);
            append_autoload_debug(format_args!(
                "save-steamid-normalize: wrote normalized GAME save path='{}' reason={reason} before=0x{before:016x} after=0x{after:016x}",
                path.display()
            ));
        }
        Err(err) => append_autoload_debug(format_args!(
            "save-steamid-normalize: FAILED to write normalized GAME save path='{}' reason={reason}: {err}",
            path.display()
        )),
    }
}

/// Redirect directory (UTF-16, NUL-free, no trailing separator) computed from the parent of
/// `ER_EFFECTS_SAVE_FILE`. Set once at init, BEFORE the CreateFileW hook is armed.
static SAVE_REDIRECT_DIR_W: OnceLock<Vec<u16>> = OnceLock::new();
/// Configured save file may be an arbitrary loose `.sl2`/`.co2` file, not staged under
/// `EldenRing/<steamid>`. It is a read-only source copied into the private native save tree; save opens
/// are redirected to that staged tree, never back to this source path.
static SAVE_DIRECT_SOURCE_FILE: OnceLock<PathBuf> = OnceLock::new();
static SAVE_DIRECT_STAGE_ROOT: OnceLock<PathBuf> = OnceLock::new();
static SAVE_DIRECT_STAGE_DONE_STEAM_ID: AtomicU64 = AtomicU64::new(0);
static SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID: AtomicU64 = AtomicU64::new(0);
static SAVE_DIRECT_STAGE_DIAG_HITS: AtomicU64 = AtomicU64::new(0);
static SAVE_DIRECT_STAGE_NO_STEAMID_HITS: AtomicU64 = AtomicU64::new(0);
static SAVE_DIRECT_STAGE_LAST_NO_STEAMID_KIND: AtomicUsize =
    AtomicUsize::new(DIRECT_STAGE_NO_STEAMID_KIND_NONE);
const DIRECT_STAGE_NO_STEAMID_KIND_NONE: usize = 0;
const DIRECT_STAGE_NO_STEAMID_KIND_ROOT: usize = 1;
const DIRECT_STAGE_NO_STEAMID_KIND_GRAPHICS: usize = 2;
const DIRECT_STAGE_NO_STEAMID_KIND_CONFIGURED_SAVE: usize = 3;
const DIRECT_STAGE_NO_STEAMID_KIND_OTHER: usize = 4;
static SAVE_REDIRECT_MODE: AtomicUsize = AtomicUsize::new(SAVE_REDIRECT_MODE_UNSET);
const SAVE_REDIRECT_MODE_UNSET: usize = 0;
const SAVE_REDIRECT_MODE_STAGED_ROOT: usize = 1;
const SAVE_REDIRECT_MODE_DIRECT_FILE: usize = 2;
const SAVE_REDIRECT_MODE_DEFAULT_USER: usize = 3;

pub(crate) fn write_save_redirect_telemetry(body: &mut String) {
    let mode = match SAVE_REDIRECT_MODE.load(Ordering::SeqCst) {
        SAVE_REDIRECT_MODE_STAGED_ROOT => "staged_root",
        SAVE_REDIRECT_MODE_DIRECT_FILE => "direct_file",
        SAVE_REDIRECT_MODE_DEFAULT_USER => "default_user_save",
        _ => "unset",
    };
    let observed_steam_id = OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst);
    let done_steam_id = SAVE_DIRECT_STAGE_DONE_STEAM_ID.load(Ordering::SeqCst);
    let in_progress_steam_id = SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.load(Ordering::SeqCst);
    let direct_source_set = SAVE_DIRECT_SOURCE_FILE.get().is_some();
    let direct_stage_root_set = SAVE_DIRECT_STAGE_ROOT.get().is_some();
    let (direct_stage_file_exists, direct_stage_file_bytes) =
        direct_stage_file_status(observed_steam_id);
    let shgfp_requests = SAVE_REDIRECT_SHGFP_APPDATA_REQUESTS.load(Ordering::SeqCst);
    let shgfp_hits = SAVE_REDIRECT_SHGFP_LOGGED.load(Ordering::SeqCst);
    let shgfp_direct_blocks = SAVE_REDIRECT_SHGFP_DIRECT_FILE_BLOCKS.load(Ordering::SeqCst);
    let shgfp_first_load_blocks = SAVE_REDIRECT_SHGFP_FIRST_LOAD_DONE_BLOCKS.load(Ordering::SeqCst);
    let shgfp_no_root_blocks = SAVE_REDIRECT_SHGFP_NO_ROOT_BLOCKS.load(Ordering::SeqCst);
    let shgfp_decision = if shgfp_hits != 0 {
        "redirected"
    } else if shgfp_direct_blocks != 0 {
        "blocked_direct_file_mode"
    } else if shgfp_first_load_blocks != 0 {
        "blocked_first_load_done"
    } else if shgfp_no_root_blocks != 0 {
        "blocked_no_redirect_root"
    } else if shgfp_requests != 0 {
        "requested_but_no_decision"
    } else {
        "not_requested"
    };
    let no_steamid_kind = direct_stage_no_steamid_kind_label(
        SAVE_DIRECT_STAGE_LAST_NO_STEAMID_KIND.load(Ordering::SeqCst),
    );
    let last_save_like_kind =
        save_path_kind_label(SAVE_CREATEFILEW_LAST_SAVE_LIKE_KIND.load(Ordering::SeqCst));
    body.push_str(&format!(
        "  \"oracle_save_redirect_mode\": \"{mode}\",\n  \"oracle_save_redirect_observed_steam_id64\": {observed_steam_id},\n  \"oracle_save_redirect_env_normalize_done\": {},\n  \"oracle_save_redirect_first_load_done\": {},\n  \"oracle_save_redirect_shgetfolderpath_decision\": \"{shgfp_decision}\",\n  \"oracle_save_redirect_shgetfolderpath_appdata_requests\": {shgfp_requests},\n  \"oracle_save_redirect_shgetfolderpath_hits\": {shgfp_hits},\n  \"oracle_save_redirect_shgetfolderpath_direct_file_blocks\": {shgfp_direct_blocks},\n  \"oracle_save_redirect_shgetfolderpath_first_load_done_blocks\": {shgfp_first_load_blocks},\n  \"oracle_save_redirect_shgetfolderpath_no_root_blocks\": {shgfp_no_root_blocks},\n  \"oracle_save_redirect_createfilew_calls\": {},\n  \"oracle_save_redirect_createfilew_diag_hits\": {},\n  \"oracle_save_redirect_createfilew_last_save_like_kind\": \"{last_save_like_kind}\",\n  \"oracle_save_redirect_createfilew_stage_steamid_dir_hits\": {},\n  \"oracle_save_redirect_createfilew_stage_save_file_hits\": {},\n  \"oracle_save_redirect_createfilew_configured_file_hits\": {},\n  \"oracle_save_redirect_query_last_save_like_kind\": \"{}\",\n  \"oracle_save_redirect_query_stage_steamid_dir_hits\": {},\n  \"oracle_save_redirect_query_stage_save_file_hits\": {},\n  \"oracle_save_redirect_query_configured_file_hits\": {},\n  \"oracle_save_redirect_redir_hits\": {},\n  \"oracle_save_redirect_sl2_query_hits\": {},\n  \"oracle_save_redirect_ntcreate_diag_hits\": {},\n  \"oracle_save_redirect_direct_source_set\": {direct_source_set},\n  \"oracle_save_redirect_direct_stage_root_set\": {direct_stage_root_set},\n  \"oracle_save_redirect_direct_stage_done_steam_id64\": {done_steam_id},\n  \"oracle_save_redirect_direct_stage_in_progress_steam_id64\": {in_progress_steam_id},\n  \"oracle_save_redirect_direct_stage_diag_hits\": {},\n  \"oracle_save_redirect_direct_stage_no_steamid_hits\": {},\n  \"oracle_save_redirect_direct_stage_last_no_steamid_kind\": \"{no_steamid_kind}\",\n  \"oracle_save_redirect_direct_stage_file_exists\": {direct_stage_file_exists},\n  \"oracle_save_redirect_direct_stage_file_bytes\": {},\n",
        SAVE_STEAM_ID_ENV_NORMALIZE_DONE.load(Ordering::SeqCst),
        SAVE_FIRST_LOAD_DONE.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_CALLS.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_DIAG_HITS.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_STAGE_STEAMID_DIR_HITS.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_STAGE_SAVE_FILE_HITS.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_CONFIGURED_FILE_HITS.load(Ordering::SeqCst),
        save_path_kind_label(SAVE_QUERY_LAST_SAVE_LIKE_KIND.load(Ordering::SeqCst)),
        SAVE_QUERY_STAGE_STEAMID_DIR_HITS.load(Ordering::SeqCst),
        SAVE_QUERY_STAGE_SAVE_FILE_HITS.load(Ordering::SeqCst),
        SAVE_QUERY_CONFIGURED_FILE_HITS.load(Ordering::SeqCst),
        SAVE_REDIRECT_HITS.load(Ordering::SeqCst),
        SAVE_SL2_QUERY_LOGGED.load(Ordering::SeqCst),
        SAVE_NTCREATE_DIAG_LOGGED.load(Ordering::SeqCst),
        SAVE_DIRECT_STAGE_DIAG_HITS.load(Ordering::SeqCst),
        SAVE_DIRECT_STAGE_NO_STEAMID_HITS.load(Ordering::SeqCst),
        direct_stage_file_bytes.map_or_else(|| "null".to_owned(), |bytes| bytes.to_string())
    ));
}

fn save_path_kind_label(kind: usize) -> &'static str {
    match kind {
        SAVE_PATH_KIND_ELDENRING_ROOT => "eldenring_root",
        SAVE_PATH_KIND_GRAPHICS_CONFIG => "graphics_config",
        SAVE_PATH_KIND_STAGE_STEAMID_DIR => "stage_steamid_dir",
        SAVE_PATH_KIND_STAGE_SAVE_FILE => "stage_save_file",
        SAVE_PATH_KIND_CONFIGURED_SAVE_FILE => "configured_save_file",
        SAVE_PATH_KIND_OTHER_SAVE_LIKE => "other_save_like",
        _ => "none",
    }
}

fn classify_save_like_createfile_path(path: &[u16]) -> usize {
    let no_steamid_kind = direct_stage_no_steamid_kind(path);
    if steam_id64_from_wide_save_path(path).is_some() {
        const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
        const CO2D: &[u16] = &[b'.' as u16, b'c' as u16, b'o' as u16, b'2' as u16];
        if wide_ends_with_ci_ascii(path, SL2D) || wide_ends_with_ci_ascii(path, CO2D) {
            SAVE_PATH_KIND_STAGE_SAVE_FILE
        } else {
            SAVE_PATH_KIND_STAGE_STEAMID_DIR
        }
    } else if no_steamid_kind == DIRECT_STAGE_NO_STEAMID_KIND_CONFIGURED_SAVE {
        SAVE_PATH_KIND_CONFIGURED_SAVE_FILE
    } else if no_steamid_kind == DIRECT_STAGE_NO_STEAMID_KIND_GRAPHICS {
        SAVE_PATH_KIND_GRAPHICS_CONFIG
    } else if no_steamid_kind == DIRECT_STAGE_NO_STEAMID_KIND_ROOT {
        SAVE_PATH_KIND_ELDENRING_ROOT
    } else {
        SAVE_PATH_KIND_OTHER_SAVE_LIKE
    }
}

fn record_save_like_createfile_path_kind(path: &[u16]) {
    let kind = classify_save_like_createfile_path(path);
    SAVE_CREATEFILEW_LAST_SAVE_LIKE_KIND.store(kind, Ordering::SeqCst);
    match kind {
        SAVE_PATH_KIND_STAGE_STEAMID_DIR => {
            SAVE_CREATEFILEW_STAGE_STEAMID_DIR_HITS.fetch_add(1, Ordering::SeqCst);
        }
        SAVE_PATH_KIND_STAGE_SAVE_FILE => {
            SAVE_CREATEFILEW_STAGE_SAVE_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        SAVE_PATH_KIND_CONFIGURED_SAVE_FILE => {
            SAVE_CREATEFILEW_CONFIGURED_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
}

fn record_save_like_query_path_kind(path: &[u16]) {
    let kind = classify_save_like_createfile_path(path);
    SAVE_QUERY_LAST_SAVE_LIKE_KIND.store(kind, Ordering::SeqCst);
    match kind {
        SAVE_PATH_KIND_STAGE_STEAMID_DIR => {
            SAVE_QUERY_STAGE_STEAMID_DIR_HITS.fetch_add(1, Ordering::SeqCst);
        }
        SAVE_PATH_KIND_STAGE_SAVE_FILE => {
            SAVE_QUERY_STAGE_SAVE_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        SAVE_PATH_KIND_CONFIGURED_SAVE_FILE => {
            SAVE_QUERY_CONFIGURED_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    }
}

fn direct_stage_no_steamid_kind_label(kind: usize) -> &'static str {
    match kind {
        DIRECT_STAGE_NO_STEAMID_KIND_ROOT => "eldenring_root",
        DIRECT_STAGE_NO_STEAMID_KIND_GRAPHICS => "graphics_config",
        DIRECT_STAGE_NO_STEAMID_KIND_CONFIGURED_SAVE => "configured_save_without_steamid",
        DIRECT_STAGE_NO_STEAMID_KIND_OTHER => "other",
        _ => "none",
    }
}

fn direct_stage_no_steamid_kind(path: &[u16]) -> usize {
    const GRAPHICS_XML: &[u16] = &[
        b'g' as u16,
        b'r' as u16,
        b'a' as u16,
        b'p' as u16,
        b'h' as u16,
        b'i' as u16,
        b'c' as u16,
        b's' as u16,
        b'c' as u16,
        b'o' as u16,
        b'n' as u16,
        b'f' as u16,
        b'i' as u16,
        b'g' as u16,
        b'.' as u16,
        b'x' as u16,
        b'm' as u16,
        b'l' as u16,
    ];
    const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
    const CO2D: &[u16] = &[b'.' as u16, b'c' as u16, b'o' as u16, b'2' as u16];
    if wide_ends_with_ci_ascii(path, GRAPHICS_XML) {
        DIRECT_STAGE_NO_STEAMID_KIND_GRAPHICS
    } else if wide_ends_with_ci_ascii(path, SL2D) || wide_ends_with_ci_ascii(path, CO2D) {
        DIRECT_STAGE_NO_STEAMID_KIND_CONFIGURED_SAVE
    } else if wide_ends_with_separator_or_eldenring(path) {
        DIRECT_STAGE_NO_STEAMID_KIND_ROOT
    } else {
        DIRECT_STAGE_NO_STEAMID_KIND_OTHER
    }
}

fn wide_ends_with_separator_or_eldenring(path: &[u16]) -> bool {
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
    let trimmed_len = path
        .iter()
        .rposition(|&c| c != b'\\' as u16 && c != b'/' as u16)
        .map_or(0, |idx| idx + 1);
    wide_ends_with_ci_ascii(&path[..trimmed_len], ELDENRING)
}

fn direct_stage_file_status(steam_id: u64) -> (bool, Option<u64>) {
    if steam_id == 0 {
        return (false, None);
    }
    let Some(root) = SAVE_DIRECT_STAGE_ROOT.get() else {
        return (false, None);
    };
    let staged_is_co2 = SAVE_DIRECT_SOURCE_FILE
        .get()
        .and_then(|source| source.extension())
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("co2"));
    let candidates = if staged_is_co2 {
        [("eldenring", "er0000.co2"), ("EldenRing", "ER0000.co2")]
    } else {
        [("eldenring", "er0000.sl2"), ("EldenRing", "ER0000.sl2")]
    };
    for (dir_name, file_name) in candidates {
        let path = root
            .join(dir_name)
            .join(steam_id.to_string())
            .join(file_name);
        if let Ok(meta) = std::fs::metadata(path)
            && meta.is_file()
        {
            return (true, Some(meta.len()));
        }
    }
    (false, None)
}
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
static SAVE_REDIRECT_SHGFP_APPDATA_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static SAVE_REDIRECT_SHGFP_DIRECT_FILE_BLOCKS: AtomicUsize = AtomicUsize::new(0);
static SAVE_REDIRECT_SHGFP_FIRST_LOAD_DONE_BLOCKS: AtomicUsize = AtomicUsize::new(0);
static SAVE_REDIRECT_SHGFP_NO_ROOT_BLOCKS: AtomicUsize = AtomicUsize::new(0);
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
static SAVE_STEAM_API_STEAM_ID_LOGGED: AtomicUsize = AtomicUsize::new(0);
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
static SAVE_CREATEFILEW_LAST_SAVE_LIKE_KIND: AtomicUsize = AtomicUsize::new(SAVE_PATH_KIND_NONE);
static SAVE_CREATEFILEW_STAGE_STEAMID_DIR_HITS: AtomicUsize = AtomicUsize::new(0);
static SAVE_CREATEFILEW_STAGE_SAVE_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
static SAVE_CREATEFILEW_CONFIGURED_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
const MISSING_SAVE_DIALOG_IDLE: usize = 0;
const MISSING_SAVE_DIALOG_PENDING: usize = 1;
const MISSING_SAVE_DIALOG_READY: usize = 2;
static MISSING_SAVE_DIALOG_STATE: AtomicUsize = AtomicUsize::new(MISSING_SAVE_DIALOG_IDLE);
static MISSING_SAVE_BLOCKED_IO_LOGGED: AtomicUsize = AtomicUsize::new(0);
static SAVE_QUERY_LAST_SAVE_LIKE_KIND: AtomicUsize = AtomicUsize::new(SAVE_PATH_KIND_NONE);

fn set_missing_save_dialog_state(state: usize) {
    MISSING_SAVE_DIALOG_STATE.store(state, Ordering::SeqCst);
}

pub(crate) fn missing_save_selection_pending() -> bool {
    MISSING_SAVE_DIALOG_STATE.load(Ordering::SeqCst) == MISSING_SAVE_DIALOG_PENDING
}

/// True after an explicit loose save source (`er-effects.toml save_file` / ER_EFFECTS_SAVE_FILE) or
/// the in-game picker has activated direct-file staging. In this mode the user's original save is a
/// read-only source and native reads/writes target our private staged `%APPDATA%` tree. The load path
/// must use the full-read chain that reads the staged file directly instead of waiting on the native
/// Continue row/profile-summary path, which can be stale/empty for loose saves.
pub(crate) fn direct_save_file_source_active() -> bool {
    SAVE_DIRECT_SOURCE_FILE.get().is_some()
}
static SAVE_QUERY_STAGE_STEAMID_DIR_HITS: AtomicUsize = AtomicUsize::new(0);
static SAVE_QUERY_STAGE_SAVE_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
static SAVE_QUERY_CONFIGURED_FILE_HITS: AtomicUsize = AtomicUsize::new(0);
const SAVE_PATH_KIND_NONE: usize = 0;
const SAVE_PATH_KIND_ELDENRING_ROOT: usize = 1;
const SAVE_PATH_KIND_GRAPHICS_CONFIG: usize = 2;
const SAVE_PATH_KIND_STAGE_STEAMID_DIR: usize = 3;
const SAVE_PATH_KIND_STAGE_SAVE_FILE: usize = 4;
const SAVE_PATH_KIND_CONFIGURED_SAVE_FILE: usize = 5;
const SAVE_PATH_KIND_OTHER_SAVE_LIKE: usize = 6;
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

enum SaveRedirectSource {
    /// Configured save is already staged under `<root>/EldenRing/<steamid>/ER0000.sl2`; preserve
    /// native directory/profile discovery by redirecting the whole save root.
    StagedRoot {
        file: PathBuf,
        steam_id: u64,
        root_w: Vec<u16>,
    },
    /// User supplied an arbitrary `.sl2`/`.co2` save file path. Copy it into a private staged native
    /// save tree; do not require the user path to mirror Elden Ring's SteamID folder layout, and never
    /// redirect gameplay writes back to the source file.
    DirectFile {
        file: PathBuf,
        stage_root: PathBuf,
        root_w: Vec<u16>,
    },
}

fn validated_save_file_path(path: PathBuf) -> Option<PathBuf> {
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() < SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES {
        return None;
    }
    Some(path)
}

fn validated_configured_save_file() -> Option<PathBuf> {
    validated_save_file_path(env_save_file_path()?)
}

fn plausible_steam_id64(value: u64) -> Option<u64> {
    (value >= 10_000_000_000_000_000 && value <= 99_999_999_999_999_999).then_some(value)
}

fn configured_active_steam_id64_env() -> Option<u64> {
    ["ER_EFFECTS_ACTIVE_STEAMID", "ER_EFFECTS_ACTIVE_STEAM_ID64"]
        .into_iter()
        .find_map(|name| {
            let raw = std::env::var(name).ok()?;
            let trimmed = raw.trim();
            let is_steam_id = (16..=20).contains(&trimmed.len())
                && trimmed.as_bytes().iter().all(u8::is_ascii_digit);
            is_steam_id
                .then(|| trimmed.parse::<u64>().ok())
                .flatten()
                .and_then(plausible_steam_id64)
        })
}

type SteamApiSteamUserV021Fn = unsafe extern "system" fn() -> *mut c_void;
type SteamApiISteamUserGetSteamIdFn = unsafe extern "system" fn(*mut c_void) -> u64;

fn steam_api_active_steam_id64() -> Option<u64> {
    let steam_user_addr =
        unsafe { module_proc(b"steam_api64.dll\0", b"SteamAPI_SteamUser_v021\0") };
    let get_steam_id_addr =
        unsafe { module_proc(b"steam_api64.dll\0", b"SteamAPI_ISteamUser_GetSteamID\0") };
    if steam_user_addr == HOOK_ORIGINAL_UNSET || get_steam_id_addr == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let steam_user: SteamApiSteamUserV021Fn = unsafe { std::mem::transmute(steam_user_addr) };
    let get_steam_id: SteamApiISteamUserGetSteamIdFn =
        unsafe { std::mem::transmute(get_steam_id_addr) };
    let iface = unsafe { steam_user() };
    if iface.is_null() {
        return None;
    }
    let steam_id = plausible_steam_id64(unsafe { get_steam_id(iface) })?;
    if SAVE_STEAM_API_STEAM_ID_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "save-override: SteamAPI active SteamID64 resolved: {steam_id}"
        ));
    }
    Some(steam_id)
}

fn configured_active_steam_id64() -> Option<(u64, &'static str)> {
    configured_active_steam_id64_env()
        .map(|steam_id| (steam_id, "early-env-active-steamid"))
        .or_else(|| {
            steam_api_active_steam_id64()
                .map(|steam_id| (steam_id, "early-steamapi-active-steamid"))
        })
}

pub(crate) fn default_save_root() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|profile| PathBuf::from(profile).join("AppData").join("Roaming"))
        })
        .map(|appdata| appdata.join("EldenRing"))
}

/// Default save file name for the active runtime mode. Seamless Co-op (ERSC) keeps co-op progress in
/// `ER0000.co2` -- a separate container from the vanilla `ER0000.sl2`. This is deliberately
/// mode-locked: a Seamless launch must not silently load a vanilla `.sl2` just because it is the only
/// appdata save present, and a vanilla launch must not silently load a Seamless `.co2`. If the active
/// mode's file is absent, default-save discovery returns "no save" and the normal missing-save picker
/// asks the user for the correct save flavor.
pub(crate) fn active_default_save_file_name() -> &'static str {
    if save_picker_seamless_mode_after_settle("active-default-save-file-name") {
        "ER0000.co2"
    } else {
        "ER0000.sl2"
    }
}

/// Accept a default-save candidate only when it holds at least one readable character. The game
/// natively creates a full-size EMPTY container on a no-save boot (28 MB, passes the size floor),
/// which must read as "no save" so the missing-save picker re-arms instead of silently entering
/// DEFAULT-USER-SAVE on a characterless file.
fn default_save_with_character(path: PathBuf) -> Option<PathBuf> {
    let bytes = fs::read(&path).ok()?;
    if save_bytes_have_any_character(&bytes) {
        return Some(path);
    }
    append_autoload_debug(format_args!(
        "save-override: default save '{}' has ZERO readable character slots (native empty container); treating as no save",
        path.display()
    ));
    None
}

fn default_save_file_for_steam_id64(steam_id: u64) -> Option<PathBuf> {
    let dir = default_save_root()?.join(steam_id.to_string());
    validated_save_file_path(dir.join(active_default_save_file_name()))
        .and_then(default_save_with_character)
}

fn default_save_file_candidates() -> Vec<(PathBuf, u64)> {
    let Some(root) = default_save_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let steam_id = entry
                .file_name()
                .to_str()
                .filter(|name| (16..=20).contains(&name.len()))
                .filter(|name| name.as_bytes().iter().all(u8::is_ascii_digit))
                .and_then(|name| name.parse::<u64>().ok())
                .and_then(plausible_steam_id64)?;
            let dir = entry.path();
            validated_save_file_path(dir.join(active_default_save_file_name()))
                .and_then(default_save_with_character)
                .map(|path| (path, steam_id))
        })
        .collect()
}

fn active_default_save_file() -> Option<(PathBuf, u64, &'static str)> {
    if let Some((steam_id, reason)) = configured_active_steam_id64() {
        return match default_save_file_for_steam_id64(steam_id) {
            Some(path) => Some((path, steam_id, reason)),
            None => {
                append_autoload_debug(format_args!(
                    "save-override: no plausible default save for active SteamID64 {steam_id} ({reason})"
                ));
                None
            }
        };
    }
    let mut candidates = default_save_file_candidates();
    if candidates.len() == 1 {
        let (path, steam_id) = candidates.remove(0);
        return Some((path, steam_id, "single-default-save-dir"));
    }
    append_autoload_debug(format_args!(
        "save-override: active default save unresolved -- active SteamID64 unavailable and plausible default save candidate count={}",
        candidates.len()
    ));
    None
}

pub(crate) fn configured_or_default_save_file() -> Option<PathBuf> {
    configured_save_file().or_else(|| active_default_save_file().map(|(path, _, _)| path))
}

fn direct_mode_native_active_save_file() -> Option<PathBuf> {
    SAVE_DIRECT_SOURCE_FILE.get()?;
    let steam_id = OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst);
    plausible_steam_id64(steam_id)?;
    ensure_direct_stage_for_steam_id(steam_id);
    Some(
        default_save_root()?
            .join(steam_id.to_string())
            .join(active_default_save_file_name()),
    )
}

/// Runtime-active save file for System->Quit character switching. A direct/picked save is a READ-ONLY
/// source: it is copied into the private redirected native save tree, and all writes must target the
/// native `%APPDATA%/EldenRing/<steamid>/ER0000.{co2|sl2}` path (which our hook redirects to that staged
/// copy). Never return `SAVE_DIRECT_SOURCE_FILE` here; that would overwrite user-provided saves.
pub(crate) fn active_save_file_for_system_quit() -> Option<PathBuf> {
    if SAVE_DIRECT_SOURCE_FILE.get().is_some() {
        return direct_mode_native_active_save_file();
    }
    configured_or_default_save_file()
}

fn staged_save_root_for_configured_file(path: &Path) -> Option<(PathBuf, u64)> {
    let mut root = PathBuf::new();
    let mut comps = path.components().peekable();
    while let Some(comp) = comps.next() {
        let text = comp.as_os_str().to_string_lossy();
        if text.eq_ignore_ascii_case("EldenRing") {
            let Some(steam_id_comp) = comps.peek() else {
                return None;
            };
            let steam_id = steam_id_comp.as_os_str().to_string_lossy();
            let is_steam_id = (16..=20).contains(&steam_id.len())
                && steam_id.as_bytes().iter().all(u8::is_ascii_digit);
            if is_steam_id {
                return steam_id
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value != 0)
                    .map(|value| (root, value));
            }
            return None;
        }
        root.push(comp);
    }
    None
}

fn save_redirect_source_for_validated_file(path: PathBuf) -> SaveRedirectSource {
    if let Some((staged_root, steam_id)) = staged_save_root_for_configured_file(&path)
        && save_file_writeback_allowed(&path)
    {
        return SaveRedirectSource::StagedRoot {
            file: path,
            steam_id,
            root_w: path_root_to_wine_wide(&staged_root),
        };
    }
    // Explicit/user-picked non-default saves are read-only sources, even if they already live under an
    // `EldenRing/<steamid>/ER0000.*` layout. The game writes only to our private staged copy.
    let stage_root = path
        .parent()
        .map(|parent| parent.join("er-effects-save-redirect-stage"))
        .unwrap_or_else(|| PathBuf::from("er-effects-save-redirect-stage"));
    SaveRedirectSource::DirectFile {
        file: path.clone(),
        root_w: path_root_to_wine_wide(&stage_root),
        stage_root,
    }
}

fn save_override_redirect_source() -> Option<SaveRedirectSource> {
    validated_configured_save_file().map(save_redirect_source_for_validated_file)
}

/// Outcome of `enforce_save_override_or_abort`. The abort path does not return.
pub(crate) enum SaveOverrideMode {
    /// Pure telemetry/observe-only: no save source required, no redirect installed.
    TelemetryOnly,
    /// A valid explicit save source was resolved; the redirect hook should be installed.
    Redirect,
    /// No explicit source was supplied; the active Steam user's default save exists and is used in place.
    DefaultUserSave,
}

fn activate_save_redirect_source(
    source: SaveRedirectSource,
    source_label: &'static str,
) -> SaveOverrideMode {
    match source {
        SaveRedirectSource::StagedRoot {
            file,
            steam_id,
            root_w,
        } => {
            OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
            normalize_env_save_file_to_known_steam_id(&file, steam_id, source_label);
            SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
            // UTF-8 Lossy: log-only decode of configured Windows wide path for probe confirmation.
            let shown = String::from_utf16_lossy(&root_w);
            let _ = SAVE_REDIRECT_DIR_W.set(root_w);
            SAVE_REDIRECT_MODE.store(SAVE_REDIRECT_MODE_STAGED_ROOT, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-override: ENFORCED -- redirecting native save root to staged root '{shown}' source={source_label}"
            ));
            SaveOverrideMode::Redirect
        }
        SaveRedirectSource::DirectFile {
            file,
            stage_root,
            root_w,
        } => {
            let _ = std::fs::create_dir_all(stage_root.join("eldenring"));
            let _ = std::fs::create_dir_all(stage_root.join("EldenRing"));
            // UTF-8 Lossy: log-only decode of configured source/stage paths for probe confirmation.
            let shown = file.display().to_string();
            let stage_shown = String::from_utf16_lossy(&root_w);
            let configured_file = file.clone();
            let explicit_steam_id = configured_active_steam_id64();
            let _ = SAVE_DIRECT_SOURCE_FILE.set(file);
            let _ = SAVE_DIRECT_STAGE_ROOT.set(stage_root);
            let _ = SAVE_REDIRECT_DIR_W.set(root_w);
            SAVE_REDIRECT_MODE.store(SAVE_REDIRECT_MODE_DIRECT_FILE, Ordering::SeqCst);
            if let Some((steam_id, reason)) = explicit_steam_id {
                OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
                normalize_env_save_file_to_known_steam_id(&configured_file, steam_id, reason);
                SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
                ensure_direct_stage_for_steam_id(steam_id);
            }
            append_autoload_debug(format_args!(
                "save-override: ENFORCED -- staging supplied save source '{shown}' into private native save root '{stage_shown}' source={source_label} active_steamid={} (source is never a write target)",
                explicit_steam_id.map(|(steam_id, _)| steam_id).unwrap_or(0)
            ));
            SaveOverrideMode::Redirect
        }
    }
}

/// Called EARLY in `DllMain` (before any save IO). Explicit save sources still install the
/// redirect hook. With no explicit source, a plausible active Steam-user default save is accepted and
/// the game reads it normally. If neither source exists, the user can choose a save file or quit.
pub(crate) fn enforce_save_override_or_abort() -> SaveOverrideMode {
    if save_override_telemetry_only() {
        append_autoload_debug(format_args!(
            "save-override: TELEMETRY-ONLY mode -- save source not enforced (loads nothing; no default-dir read for a character)"
        ));
        return SaveOverrideMode::TelemetryOnly;
    }
    if configured_save_file().is_none()
        && let Some((file, steam_id, reason)) = active_default_save_file()
    {
        OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
        SAVE_REDIRECT_MODE.store(SAVE_REDIRECT_MODE_DEFAULT_USER, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-override: DEFAULT-USER-SAVE -- no ER_EFFECTS_SAVE_FILE/save_file configured; using active SteamID64 {steam_id} ({reason}) default save '{}' with no redirect",
            file.display()
        ));
        return SaveOverrideMode::DefaultUserSave;
    }
    if let Some(source) = save_override_redirect_source() {
        return activate_save_redirect_source(source, "early-enforced-configured-save");
    }
    append_autoload_debug(format_args!(
        "save-override: no explicit save_file/ER_EFFECTS_SAVE_FILE and no readable active default {} (>= {} bytes). config_error={}. Arming the IN-GAME missing-save picker: the title boots to its native no-save menu and the 05_010 file browser presents itself (save_picker_menu.rs); world entry stays denied until a save is picked.",
        active_default_save_file_name(),
        SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES,
        runtime_config_error().unwrap_or_else(|| "none".to_owned())
    ));
    set_missing_save_dialog_state(MISSING_SAVE_DIALOG_PENDING);
    SaveOverrideMode::Redirect
}

/// Picker-mode helper for user-facing save selection. ERSC can register after our DllMain, so picker
/// mode first honors an explicit launcher/profile hint for known Seamless launches, then falls back to
/// the sticky runtime module latch. No sleep/polling: picker mode must come from a concrete signal.
// ENV-GATE RATIONALE: ER_EFFECTS_SAVE_MODE_HINT is set by the user-facing launcher/profile wrapper
// to disambiguate Seamless `.co2` vs vanilla `.sl2` before `ersc.dll` is guaranteed to be
// PEB-registered; without that concrete launch-mode signal, the pre-save missing-save picker can
// expose the wrong save flavor and stage a file the active runtime will never own.
pub(crate) fn save_picker_seamless_mode_after_settle(reason: &str) -> bool {
    // DE-GATED (deprecate-env-marker-gate-allowlists-2026-07-19): the ER_EFFECTS_SAVE_MODE_HINT env
    // override that forced Seamless `.co2` vs vanilla `.sl2` is removed -- env feature gates are
    // forbidden. Picker flavor now comes solely from the real ERSC runtime module latch
    // (`seamless_coop_loaded()`), which this `_after_settle` path reads once ERSC has had time to
    // PEB-register. (Compatibility flag: early-Seamless disambiguation now depends on the latch
    // being populated by settle time -- see deprecate report; verify on a Seamless launch.)
    let seamless = crate::telemetry::seamless_coop_loaded();
    append_autoload_debug(format_args!(
        "save-override: save-picker mode from ERSC module latch seamless={seamless} reason={reason}"
    ));
    seamless
}

/// Complete the missing-save selection from the IN-GAME title picker (menu thread). Validates the
/// picked container (size floor + BND4 parse -- stronger than the old OS flow's size-only check),
/// persists the picked directory, activates the save-redirect source, installs the Win32 redirect
/// hooks synchronously (idempotent -- the install is Once-guarded), and releases every waiter on
/// the missing-save gate. Returns false (state unchanged, picker stays up) on an invalid pick.
pub(crate) fn complete_missing_save_selection_from_picker(path: &Path) -> bool {
    let Some(validated) = validated_save_file_path(path.to_path_buf()) else {
        append_autoload_debug(format_args!(
            "save-override: title picker rejected non-plausible save '{}' (missing or under {} bytes)",
            path.display(),
            SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES
        ));
        return false;
    };
    match fs::read(&validated) {
        Ok(bytes) if er_save_loader::bnd4::parse_entries(&bytes).is_ok() => {}
        Ok(bytes) => {
            append_autoload_debug(format_args!(
                "save-override: title picker rejected non-BND4 file '{}' len={}",
                validated.display(),
                bytes.len()
            ));
            return false;
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "save-override: title picker could not read '{}': {err}",
                validated.display()
            ));
            return false;
        }
    }
    if autoupdate_preferred_picker_dir_enabled()
        && let Some(dir) = validated.parent().filter(|dir| !dir.as_os_str().is_empty())
    {
        remember_preferred_save_picker_dir(dir);
    }
    let source = save_redirect_source_for_validated_file(validated.clone());
    let _ = activate_save_redirect_source(source, "title-picker-selection");
    install_save_redirect_hooks();
    set_missing_save_dialog_state(MISSING_SAVE_DIALOG_READY);
    append_autoload_debug(format_args!(
        "save-override: title picker selected save '{}'; redirect active, missing-save gate released",
        validated.display()
    ));
    true
}

/// Diagnostic-only observer for save-like IO while the missing-save selection is pending. The
/// IN-GAME picker flow REQUIRES this IO to proceed: the title must complete its natural no-save
/// boot (empty ProfileSummary, interactive menu) for the 05_010 file browser to present itself --
/// blocking here re-creates the input-dead title the old OS dialog existed to paper over. The
/// pick later installs/activates the redirect and fires a title reload, so nothing read during
/// the pending window is ever committed.
fn wait_for_missing_save_dialog_if_pending(path: &[u16]) {
    if MISSING_SAVE_DIALOG_STATE.load(Ordering::SeqCst) != MISSING_SAVE_DIALOG_PENDING {
        return;
    }
    let hit = MISSING_SAVE_BLOCKED_IO_LOGGED.fetch_add(1, Ordering::SeqCst);
    if hit < 8 {
        // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
        let p = String::from_utf16_lossy(path);
        append_autoload_debug(format_args!(
            "save-override: native save-file IO proceeding while the in-game missing-save picker is pending path='{p}'"
        ));
    }
}

fn is_save_file_or_backup_path(path: &[u16]) -> bool {
    const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
    const CO2D: &[u16] = &[b'.' as u16, b'c' as u16, b'o' as u16, b'2' as u16];
    const BAKD: &[u16] = &[b'.' as u16, b'b' as u16, b'a' as u16, b'k' as u16];
    wide_ends_with_ci_ascii(path, SL2D)
        || wide_ends_with_ci_ascii(path, CO2D)
        || wide_ends_with_ci_ascii(path, BAKD)
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
fn ensure_direct_stage_for_requested_path(path: &[u16]) {
    let Some(root) = SAVE_DIRECT_STAGE_ROOT.get() else {
        return;
    };
    let Some(steam_id) = steam_id64_from_wide_save_path(path) else {
        let kind = direct_stage_no_steamid_kind(path);
        SAVE_DIRECT_STAGE_NO_STEAMID_HITS.fetch_add(1, Ordering::SeqCst);
        SAVE_DIRECT_STAGE_LAST_NO_STEAMID_KIND.store(kind, Ordering::SeqCst);
        let hit = SAVE_DIRECT_STAGE_DIAG_HITS.fetch_add(1, Ordering::SeqCst);
        if hit < 8 {
            // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
            let shown = String::from_utf16_lossy(path);
            let kind_label = direct_stage_no_steamid_kind_label(kind);
            append_autoload_debug(format_args!(
                "save-override: direct-file stage pending -- no SteamID64 in {kind_label} requested path '{shown}'"
            ));
        }
        let _ = std::fs::create_dir_all(root.join("eldenring"));
        let _ = std::fs::create_dir_all(root.join("EldenRing"));
        return;
    };
    ensure_direct_stage_for_steam_id(steam_id);
}

fn make_file_writable(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

fn remove_file_for_overwrite(path: &Path) -> std::io::Result<()> {
    make_file_writable(path);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn copy_save_for_overwrite(source: &Path, target: &Path, steam_id: u64) -> std::io::Result<u64> {
    let mut bytes = std::fs::read(source)?;
    match er_save_loader::bnd4::normalize_steam_id_in_place(&mut bytes, steam_id) {
        Ok(report) if report.changed() => append_autoload_debug(format_args!(
            "save-override: direct-file staging normalized private copy source='{}' target='{}' steam_id={steam_id} char_patched={} user_data10_patched={} md5_rewritten={}",
            source.display(),
            target.display(),
            report.character_slots_patched,
            report.user_data10_patched,
            report.md5_rewritten
        )),
        Ok(_) => {}
        Err(err) => append_autoload_debug(format_args!(
            "save-override: direct-file staging normalization skipped source='{}' target='{}' steam_id={steam_id}: {err:?}",
            source.display(),
            target.display()
        )),
    }
    remove_file_for_overwrite(target)?;
    std::fs::write(target, &bytes)?;
    make_file_writable(target);
    Ok(bytes.len() as u64)
}

fn ensure_direct_stage_for_steam_id(steam_id: u64) {
    let Some(source) = SAVE_DIRECT_SOURCE_FILE.get() else {
        let hit = SAVE_DIRECT_STAGE_DIAG_HITS.fetch_add(1, Ordering::SeqCst);
        if hit < 8 {
            append_autoload_debug(format_args!(
                "save-override: direct-file stage pending -- no configured source yet for SteamID64 {steam_id}"
            ));
        }
        return;
    };
    let Some(root) = SAVE_DIRECT_STAGE_ROOT.get() else {
        let hit = SAVE_DIRECT_STAGE_DIAG_HITS.fetch_add(1, Ordering::SeqCst);
        if hit < 8 {
            append_autoload_debug(format_args!(
                "save-override: direct-file stage pending -- no stage root yet for SteamID64 {steam_id}"
            ));
        }
        return;
    };
    let prior = SAVE_DIRECT_STAGE_DONE_STEAM_ID.load(Ordering::SeqCst);
    if prior == steam_id {
        return;
    }
    match SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.compare_exchange(
        0,
        steam_id,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        Ok(_) => {}
        Err(in_progress) if in_progress == steam_id => return,
        Err(in_progress) => {
            let hit = SAVE_DIRECT_STAGE_DIAG_HITS.fetch_add(1, Ordering::SeqCst);
            if hit < 16 {
                append_autoload_debug(format_args!(
                    "save-override: direct-file stage deferred for SteamID64 {steam_id}; SteamID64 {in_progress} already staging"
                ));
            }
            return;
        }
    }
    let hit = SAVE_DIRECT_STAGE_DIAG_HITS.fetch_add(1, Ordering::SeqCst);
    if hit < 16 {
        append_autoload_debug(format_args!(
            "save-override: direct-file staging begin for SteamID64 {steam_id}: '{}' -> root '{}'",
            source.display(),
            root.display()
        ));
    }
    let staged_is_co2 = source
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("co2"));
    let staged_basename_lower = if staged_is_co2 {
        "er0000.co2"
    } else {
        "er0000.sl2"
    };
    let staged_basename_native = if staged_is_co2 {
        "ER0000.co2"
    } else {
        "ER0000.sl2"
    };
    let lower_dir = root.join("eldenring").join(steam_id.to_string());
    let native_dir = root.join("EldenRing").join(steam_id.to_string());
    for dir in [&lower_dir, &native_dir] {
        if let Err(err) = std::fs::create_dir_all(dir) {
            append_autoload_debug(format_args!(
                "save-override: direct-file stage failed creating '{}': {err}",
                dir.display()
            ));
            SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.store(0, Ordering::SeqCst);
            return;
        }
    }
    let lower_target = lower_dir.join(staged_basename_lower);
    let native_target = native_dir.join(staged_basename_native);
    match copy_save_for_overwrite(source, &lower_target, steam_id) {
        Ok(lower_bytes) => match copy_save_for_overwrite(source, &native_target, steam_id) {
            Ok(native_bytes) => {
                SAVE_DIRECT_STAGE_DONE_STEAM_ID.store(steam_id, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-override: direct-file staged lower={} native={} bytes for SteamID64 {steam_id}: '{}' -> '{}' + '{}'",
                    lower_bytes,
                    native_bytes,
                    source.display(),
                    lower_target.display(),
                    native_target.display()
                ));
            }
            Err(err) => append_autoload_debug(format_args!(
                "save-override: direct-file native stage copy failed for SteamID64 {steam_id}: '{}' -> '{}': {err}",
                source.display(),
                native_target.display()
            )),
        },
        Err(err) => append_autoload_debug(format_args!(
            "save-override: direct-file stage copy failed for SteamID64 {steam_id}: '{}' -> '{}': {err}",
            source.display(),
            lower_target.display()
        )),
    }
    SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.store(0, Ordering::SeqCst);
}

fn wide_with_nul(path: &[u16]) -> Vec<u16> {
    let mut out = path.to_vec();
    out.push(0);
    out
}

/// If `path` is anywhere under the game's `%APPDATA%\Roaming\EldenRing` save root, return a
/// redirected (NUL-terminated) wide path. None = not the save root.
///
/// Configured saves may be arbitrary loose files. For full save-discovery compatibility, directory
/// opens/existence checks still redirect to our private staged `EldenRing\<steamid>` tree, populated
/// from the configured file when the native path reveals the active SteamID. Actual `.sl2`/`.co2`
/// opens redirect to the configured file itself so users do NOT need to stage their path under
/// `EldenRing` or include a SteamID folder.
fn save_redirect_path(path: &[u16]) -> Option<Vec<u16>> {
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
    // Always learn the native `<steamid>` segment from save-like paths; this is the safest
    // current-account oracle because the native save-dir builder already called Steam before the path
    // reached our hook. The redirect decision below is still anchored on `Roaming` to avoid loops.
    observe_steam_id64_from_save_path(path);
    // Anchor on `Roaming` + `EldenRing` so a coincidental "eldenring" elsewhere -- and our already-
    // redirected target (`Z:\...\save\EldenRing\...`, no "Roaming") -- never re-redirects.
    if !wide_contains_ci_ascii(path, ROAMING) {
        return None;
    }
    let idx = wide_find_ci_ascii(path, ELDENRING)?;
    // Direct-file mode stages the selected source into the private native save tree. Do NOT redirect
    // save-file or .bak opens to `SAVE_DIRECT_SOURCE_FILE`; reads and writes must hit the staged copy
    // so readonly/user-provided source saves are never modified by gameplay or profile switching.
    let root = SAVE_REDIRECT_DIR_W.get()?;
    let suffix = &path[idx..]; // "EldenRing\<id>\ER0000.sl2" (or "EldenRing\" for the dir open)
    ensure_direct_stage_for_requested_path(path);
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
        // Observe/stage before any redirect decision. Direct-file mode needs the native path builder's
        // `<steamid>` component to populate the private discovery tree, and some paths are diagnostic
        // only (not redirected) but still carry the account id.
        observe_steam_id64_from_save_path(path);
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
        if save_like {
            record_save_like_createfile_path_kind(path);
        }
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
        if is_save_file || wide_ends_with_ci_ascii(path, BAKD) {
            wait_for_missing_save_dialog_if_pending(path);
        }
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
            .map(|path| {
                if is_save_file_or_backup_path(path) {
                    wait_for_missing_save_dialog_if_pending(path);
                }
                path
            })
            .and_then(save_redirect_path)
    };
    let new_red = {
        let len = unsafe { wide_len(new_file) };
        (len != 0)
            .then(|| unsafe { std::slice::from_raw_parts(new_file, len) })
            .map(|path| {
                if is_save_file_or_backup_path(path) {
                    wait_for_missing_save_dialog_if_pending(path);
                }
                path
            })
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
    if wide_contains_ci_ascii(path, ELDENRING_SEG) || wide_contains_ci_ascii(path, ER0000) {
        record_save_like_query_path_kind(path);
    }
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
