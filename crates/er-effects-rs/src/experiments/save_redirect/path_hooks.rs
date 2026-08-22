use std::{
    ffi::c_void,
    fs,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use er_game_base::fnv1a::fnv1a64;
use er_save_redirect::{
    DIRECT_STAGE_ROOT_DIR_NAME, DirectStageNoSteamIdKind, DirectStageRequestPlan,
    SAVE_REDIRECT_ORIG_COPYFILEW, SAVE_REDIRECT_ORIG_CREATEFILEW, SAVE_REDIRECT_ORIG_FINDFIRSTW,
    SAVE_REDIRECT_ORIG_GETATTREXW, SAVE_REDIRECT_ORIG_GETATTRW, SaveDetourDepth,
    SaveHookInstallState, SavePathKind, SavePathTelemetryBucket, StagedEntryFate,
    TerminalRejectionGuard, TerminalRejectionObservation, classify_copyfile_endpoint,
    createfile_diag_hit_should_log, dedupe_dirs_by_identity, default_save_file_path,
    direct_stage_case_dirs, is_inside_direct_stage_root, plan_create_file_open,
    plan_direct_stage_request, plan_save_path_telemetry, plan_save_query_path,
    plausible_steam_id64, probe_direct_stage_file_status,
    redirect_wide_save_path_with_side_effects, save_detour_disk_io_allowed, save_file_is_readonly,
    staged_entry_fate, steam_id64_from_dir_name, steam_id64_from_wide_save_path,
};
use er_telemetry::counters::{
    SAVE_DIRECT_STAGE_CONTAINERS_WRITTEN, SAVE_DIRECT_STAGE_STALE_REMOVE_FAILED,
    SAVE_DIRECT_STAGE_STALE_REMOVED, SAVE_REDIRECT_DETOUR_MAX_DEPTH,
    SAVE_REDIRECT_DETOUR_REENTRANT_PASSTHROUGHS,
};
use windows::{Win32::System::LibraryLoader::GetModuleHandleA, core::PCSTR};

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
// WHICH PICKER that is comes from `er-effects.toml`'s `os_native_save_picker`, resolved -- like
// every other picker open -- in `save_picker_surface.rs`. Default off: the DLL-drawn overlay
// browser (`gpu_readback/save_picker_overlay.rs`), which has NO cancel; BACK is navigation and the
// user chooses a save or closes the game themselves. On: the OS common file dialog, opened by
// `startup_hooks/save_picker_boot.rs` on a thread of ours, whose Cancel button is where the
// "Cancel -> exit" half of the contract above is actually implemented (`ExitProcess(0)`, the same
// clean kill the in-world Return to Desktop performs).
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

/// Exact byte length of a real ER0000.sl2/.co2. The shared save-redirect core owns this invariant
/// so product and standalone planning reject the same impossible files.
pub(crate) const SAVE_OVERRIDE_EXPECTED_BYTES: u64 = er_save_redirect::EXPECTED_SAVE_FILE_BYTES;

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

pub(crate) use er_telemetry::counters::OBSERVED_ACTIVE_STEAM_ID64;

pub(super) fn observe_steam_id64_from_save_path(path: &[u16]) {
    if let Some(steam_id) = steam_id64_from_wide_save_path(path) {
        OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
        // Latching the observed id is a pure store and always safe. The two calls below are NOT:
        // both open files, which re-enters the detour this observation was made from. They are
        // individually guarded (`save_detour_disk_io_allowed`), so a nested observation records
        // the id and does nothing else -- and the outer one, once it unwinds, still performs the
        // work. This is the SEED half of the 2026-07-30 recursion: this call site is reached from
        // the CreateFileW detour before its own diagnostics, which is why the seeding open never
        // appeared in the log.
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

fn save_file_writeback_allowed(path: &Path) -> bool {
    let default_root = default_save_root();
    er_save_redirect::save_file_writeback_allowed(path, default_root.as_deref())
}

fn normalize_env_save_file_to_known_steam_id(path: &Path, steam_id: u64, reason: &str) {
    let Ok(mut bytes) = fs::read(path) else {
        append_autoload_debug(format_args!(
            "save-steamid-normalize: failed to read env file for early normalize reason={reason} path='{}'",
            path.display()
        ));
        return;
    };
    let before = fnv1a64(&bytes);
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
                    let after = fnv1a64(&bytes);
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

/// One-shot latch for the "no configured save file" line above (see its comment).
pub(crate) static SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn normalize_env_save_file_to_active_steam_id_once(base: usize, reason: &str) {
    if SAVE_STEAM_ID_ENV_NORMALIZE_DONE.load(Ordering::SeqCst) != 0
        || OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst) == 0
    {
        return;
    }
    // This one-shot reads a save container off disk, and both of its call sites are inside a
    // `CreateFileW`/`NtCreateFile` detour -- so the read re-enters the caller. Refuse from a nested
    // context; the outer detour entry performs it once its own I/O has unwound.
    if !save_detour_disk_io_allowed() {
        return;
    }
    let Some(path) = configured_save_file() else {
        // FIRST OCCURRENCE ONLY. There is no configured save file in default (game-owned APPDATA)
        // mode, and since save-game-flow WP3 the CreateFileW detour is installed in EVERY mode --
        // so this call site now runs on every save-container open. One line says everything; a
        // line per open is the unbounded-repetition noise the log policy forbids.
        if SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "save-steamid-normalize: no configured save file for one-shot disk normalize reason={reason} (logged once)"
            ));
        }
        return;
    };
    // CLAIM the one-shot BEFORE the read, not after it succeeds. The old order left the latch at 0
    // for the whole duration of `fs::read`, so the open that read re-entered this function with the
    // one-shot still unclaimed -- the recursion edge itself. Claiming first also means a read that
    // FAILS disables the normalize permanently, which is the honest semantic: the configured path is
    // fixed for the process (env/TOML, never rewritten at runtime), so a path that cannot be read now
    // cannot be read later, and retrying it on every save-container open is pure re-entrant waste.
    // `compare_exchange` rather than a store so two threads cannot both run the body.
    if SAVE_STEAM_ID_ENV_NORMALIZE_DONE
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let Ok(mut bytes) = fs::read(&path) else {
        append_autoload_debug(format_args!(
            "save-steamid-normalize: failed to read configured save file for one-shot disk normalize reason={reason} path='{}' (one-shot consumed; not retried)",
            path.display()
        ));
        return;
    };
    let before = fnv1a64(&bytes);
    let Some(report) = normalize_save_bytes_to_active_steam_id(base, &mut bytes, reason) else {
        return;
    };
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
            let after = fnv1a64(&bytes);
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
pub(super) static SAVE_REDIRECT_DIR_W: OnceLock<Vec<u16>> = OnceLock::new();
/// Configured save file may be an arbitrary loose `.sl2`/`.co2` file, not staged under
/// `EldenRing/<steamid>`. It is a read-only source copied into the private native save tree; save opens
/// are redirected to that staged tree, never back to this source path.
static SAVE_DIRECT_SOURCE_FILE: OnceLock<PathBuf> = OnceLock::new();
static SAVE_DIRECT_STAGE_ROOT: OnceLock<PathBuf> = OnceLock::new();
pub(crate) use er_telemetry::counters::SAVE_DIRECT_STAGE_DIAG_HITS;
pub(crate) use er_telemetry::counters::SAVE_DIRECT_STAGE_DONE_STEAM_ID;
pub(crate) use er_telemetry::counters::SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID;
pub(crate) use er_telemetry::counters::SAVE_DIRECT_STAGE_NO_STEAMID_HITS;
static SAVE_DIRECT_STAGE_LAST_NO_STEAMID_KIND: AtomicUsize =
    AtomicUsize::new(DirectStageNoSteamIdKind::None.as_usize());
static SAVE_REDIRECT_MODE: AtomicUsize = AtomicUsize::new(SAVE_REDIRECT_MODE_UNSET);
const SAVE_REDIRECT_MODE_UNSET: usize = 0;
const SAVE_REDIRECT_MODE_STAGED_ROOT: usize = 1;
const SAVE_REDIRECT_MODE_DIRECT_FILE: usize = 2;
const SAVE_REDIRECT_MODE_DEFAULT_USER: usize = 3;

/// Terminal own-load save-resolution rejection. Once the picker gate has released, a missing active
/// container cannot become valid without replacing the staged source, which this process cannot do
/// (`SAVE_DIRECT_SOURCE_FILE` is deliberately write-once). The first rejection therefore fails
/// closed; a second identical observation is a recurrence bug and sets a nonzero semaphore.
static OWN_LOAD_SAVE_REJECTION: TerminalRejectionGuard = TerminalRejectionGuard::new();
static OWN_LOAD_SAVE_REJECTION_GUARD_CHECKS: AtomicU64 = AtomicU64::new(0);
static OWN_LOAD_SAVE_REJECTION_PROBE_ARMED: AtomicUsize = AtomicUsize::new(0);
static OWN_LOAD_SAVE_REJECTION_PROBE_FIRED: AtomicUsize = AtomicUsize::new(0);
static OWN_LOAD_SAVE_REJECTION_PROBE_EXPECTED_FINGERPRINT: AtomicU64 = AtomicU64::new(0);
const OWN_LOAD_UNRESOLVABLE_PROBE_ENV: &str = "ER_EFFECTS_PROBE_OWN_LOAD_UNRESOLVABLE";

pub(crate) fn own_load_save_rejection_terminal() -> bool {
    OWN_LOAD_SAVE_REJECTION_GUARD_CHECKS.fetch_add(1, Ordering::SeqCst);
    OWN_LOAD_SAVE_REJECTION.is_terminal()
}

pub(crate) fn own_load_save_rejection_fingerprint() -> u64 {
    OWN_LOAD_SAVE_REJECTION.fingerprint()
}

pub(crate) fn own_load_save_repeated_identical_rejections() -> u64 {
    OWN_LOAD_SAVE_REJECTION.repeated_identical()
}

pub(crate) fn record_own_load_save_rejection(fingerprint: u64) -> TerminalRejectionObservation {
    OWN_LOAD_SAVE_REJECTION.record(fingerprint)
}

pub(crate) fn own_load_save_rejection_signature(dir: &Path, candidates: &[&str]) -> u64 {
    let mut identity = dir.as_os_str().to_string_lossy().into_owned();
    for candidate in candidates {
        identity.push('|');
        identity.push_str(candidate);
    }
    fnv1a64(identity.as_bytes()).max(1)
}

/// Arm and fire the deterministic YK0J runtime proof seam exactly once.
///
/// This is diagnostic-only: it makes the Rust-side own-load resolver reject the configured source
/// while the game's native save path continues reading the private staged copy. The configured/user
/// source remains read-only, the active target remains the only write target, and the run can still
/// prove a live player after exercising the fail-closed transition.
pub(crate) fn own_load_save_rejection_probe(
    source: &Path,
    candidates: &[&str],
) -> Option<(u64, TerminalRejectionObservation)> {
    // ENV-GATE RATIONALE: targeted runtime proof must deterministically exercise the otherwise
    // invariant-violation-only rejection without corrupting/removing a real save or adding sleeps.
    if !matches!(
        std::env::var(OWN_LOAD_UNRESOLVABLE_PROBE_ENV).as_deref(),
        Ok("1")
    ) || !direct_save_file_source_active()
        || missing_save_selection_pending()
    {
        return None;
    }
    let fingerprint = own_load_save_rejection_signature(source, candidates);
    OWN_LOAD_SAVE_REJECTION_PROBE_ARMED.store(1, Ordering::SeqCst);
    OWN_LOAD_SAVE_REJECTION_PROBE_EXPECTED_FINGERPRINT.store(fingerprint, Ordering::SeqCst);
    if OWN_LOAD_SAVE_REJECTION_PROBE_FIRED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return None;
    }
    Some((fingerprint, record_own_load_save_rejection(fingerprint)))
}

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
        "  \"oracle_save_redirect_mode\": \"{mode}\",\n  \"oracle_save_redirect_observed_steam_id64\": {observed_steam_id},\n  \"oracle_save_redirect_env_normalize_done\": {},\n  \"oracle_save_redirect_first_load_done\": {},\n  \"oracle_save_redirect_shgetfolderpath_decision\": \"{shgfp_decision}\",\n  \"oracle_save_redirect_shgetfolderpath_appdata_requests\": {shgfp_requests},\n  \"oracle_save_redirect_shgetfolderpath_hits\": {shgfp_hits},\n  \"oracle_save_redirect_shgetfolderpath_direct_file_blocks\": {shgfp_direct_blocks},\n  \"oracle_save_redirect_shgetfolderpath_first_load_done_blocks\": {shgfp_first_load_blocks},\n  \"oracle_save_redirect_shgetfolderpath_no_root_blocks\": {shgfp_no_root_blocks},\n  \"oracle_save_redirect_createfilew_calls\": {},\n  \"oracle_save_redirect_createfilew_diag_hits\": {},\n  \"oracle_save_redirect_createfilew_max_depth\": {},\n  \"oracle_save_redirect_createfilew_reentrant_passthroughs\": {},\n  \"oracle_save_redirect_createfilew_last_save_like_kind\": \"{last_save_like_kind}\",\n  \"oracle_save_redirect_createfilew_stage_steamid_dir_hits\": {},\n  \"oracle_save_redirect_createfilew_stage_save_file_hits\": {},\n  \"oracle_save_redirect_createfilew_configured_file_hits\": {},\n  \"oracle_save_redirect_query_last_save_like_kind\": \"{}\",\n  \"oracle_save_redirect_query_stage_steamid_dir_hits\": {},\n  \"oracle_save_redirect_query_stage_save_file_hits\": {},\n  \"oracle_save_redirect_query_configured_file_hits\": {},\n  \"oracle_save_redirect_redir_hits\": {},\n  \"oracle_save_redirect_sl2_query_hits\": {},\n  \"oracle_save_redirect_ntcreate_diag_hits\": {},\n  \"oracle_save_redirect_direct_source_set\": {direct_source_set},\n  \"oracle_save_redirect_direct_stage_root_set\": {direct_stage_root_set},\n  \"oracle_save_redirect_direct_stage_done_steam_id64\": {done_steam_id},\n  \"oracle_save_redirect_direct_stage_in_progress_steam_id64\": {in_progress_steam_id},\n  \"oracle_save_redirect_direct_stage_diag_hits\": {},\n  \"oracle_save_redirect_direct_stage_no_steamid_hits\": {},\n  \"oracle_save_redirect_direct_stage_last_no_steamid_kind\": \"{no_steamid_kind}\",\n  \"oracle_save_redirect_direct_stage_file_exists\": {direct_stage_file_exists},\n  \"oracle_save_redirect_direct_stage_file_bytes\": {},\n  \"oracle_save_redirect_direct_stage_containers_written\": {},\n  \"oracle_save_redirect_direct_stage_stale_removed\": {},\n  \"oracle_save_redirect_direct_stage_stale_remove_failed\": {},\n  \"oracle_own_load_save_rejection_state\": {},\n  \"oracle_own_load_save_rejection_fingerprint\": {},\n  \"oracle_own_load_save_rejection_attempts\": {},\n  \"oracle_own_load_save_rejection_guard_checks\": {},\n  \"oracle_own_load_save_rejection_probe_armed\": {},\n  \"oracle_own_load_save_rejection_probe_fired\": {},\n  \"oracle_own_load_save_rejection_probe_expected_fingerprint\": {},\n  \"oracle_own_load_save_repeated_identical_rejections\": {},\n  \"oracle_own_load_save_repeated_different_rejections\": {},\n",
        SAVE_STEAM_ID_ENV_NORMALIZE_DONE.load(Ordering::SeqCst),
        SAVE_FIRST_LOAD_DONE.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_CALLS.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_DIAG_HITS.load(Ordering::SeqCst),
        // THE stack-overflow semaphore. > 2 means a save-redirect detour re-entered itself without
        // being passed through -- the unbounded recursion of 2026-07-30, which manifests as the
        // game's main thread simply exiting with an empty crash log. See `reentry.rs`.
        SAVE_REDIRECT_DETOUR_MAX_DEPTH.load(Ordering::SeqCst),
        SAVE_REDIRECT_DETOUR_REENTRANT_PASSTHROUGHS.load(Ordering::SeqCst),
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
        direct_stage_file_bytes.map_or_else(|| "null".to_owned(), |bytes| bytes.to_string()),
        SAVE_DIRECT_STAGE_CONTAINERS_WRITTEN.load(Ordering::SeqCst),
        SAVE_DIRECT_STAGE_STALE_REMOVED.load(Ordering::SeqCst),
        // THE stale-serve semaphore: nonzero means a leftover container survived the sweep and the
        // game may open it instead of the configured source.
        SAVE_DIRECT_STAGE_STALE_REMOVE_FAILED.load(Ordering::SeqCst),
        usize::from(OWN_LOAD_SAVE_REJECTION.is_terminal()),
        OWN_LOAD_SAVE_REJECTION.fingerprint(),
        OWN_LOAD_SAVE_REJECTION.attempts(),
        OWN_LOAD_SAVE_REJECTION_GUARD_CHECKS.load(Ordering::SeqCst),
        OWN_LOAD_SAVE_REJECTION_PROBE_ARMED.load(Ordering::SeqCst),
        OWN_LOAD_SAVE_REJECTION_PROBE_FIRED.load(Ordering::SeqCst),
        OWN_LOAD_SAVE_REJECTION_PROBE_EXPECTED_FINGERPRINT.load(Ordering::SeqCst),
        // Both recurrence fields MUST remain zero. The first valid fail-closed transition publishes
        // state=1, a nonzero fingerprint and attempts=1; any later resolver entry makes the defect
        // machine-readable instead of silently churning.
        OWN_LOAD_SAVE_REJECTION.repeated_identical(),
        OWN_LOAD_SAVE_REJECTION.repeated_different()
    ));
}

fn save_path_kind_label(kind: usize) -> &'static str {
    SavePathKind::from_usize(kind).label()
}

fn record_save_like_createfile_path_kind(path: &[u16]) {
    let plan = plan_save_path_telemetry(path);
    SAVE_CREATEFILEW_LAST_SAVE_LIKE_KIND.store(plan.kind.as_usize(), Ordering::SeqCst);
    match plan.bucket {
        Some(SavePathTelemetryBucket::StageSteamIdDir) => {
            SAVE_CREATEFILEW_STAGE_STEAMID_DIR_HITS.fetch_add(1, Ordering::SeqCst);
        }
        Some(SavePathTelemetryBucket::StageSaveFile) => {
            SAVE_CREATEFILEW_STAGE_SAVE_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        Some(SavePathTelemetryBucket::ConfiguredSaveFile) => {
            SAVE_CREATEFILEW_CONFIGURED_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        None => {}
    }
}

fn record_save_like_query_path_kind(path: &[u16]) {
    let plan = plan_save_path_telemetry(path);
    SAVE_QUERY_LAST_SAVE_LIKE_KIND.store(plan.kind.as_usize(), Ordering::SeqCst);
    match plan.bucket {
        Some(SavePathTelemetryBucket::StageSteamIdDir) => {
            SAVE_QUERY_STAGE_STEAMID_DIR_HITS.fetch_add(1, Ordering::SeqCst);
        }
        Some(SavePathTelemetryBucket::StageSaveFile) => {
            SAVE_QUERY_STAGE_SAVE_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        Some(SavePathTelemetryBucket::ConfiguredSaveFile) => {
            SAVE_QUERY_CONFIGURED_FILE_HITS.fetch_add(1, Ordering::SeqCst);
        }
        None => {}
    }
}

fn direct_stage_no_steamid_kind_label(kind: usize) -> &'static str {
    DirectStageNoSteamIdKind::from_usize(kind).label()
}

/// Report the staged container the runtime will actually LOAD, resolved from the ACTIVE MODE's
/// candidate list. Probing by the SOURCE file's extension made this oracle confirm a staging that
/// had in fact written a different container (2026-08-11).
fn direct_stage_file_status(steam_id: u64) -> (bool, Option<u64>) {
    let status = probe_direct_stage_file_status(
        SAVE_DIRECT_STAGE_ROOT.get().map(PathBuf::as_path),
        active_default_save_file_names(),
        steam_id,
    );
    (status.exists, status.bytes)
}
/// Save-existence-check redirects: the game stats/enumerates the save file BEFORE opening it; if
/// these hit the (wiped) default dir the game concludes "no save" and never CreateFileW's it.
///
/// PRIMARY redirect: the save-dir builder (FUN_140e0e680) calls SHGetFolderPathW(CSIDL_APPDATA) to
/// get %APPDATA%, then formats `%APPDATA%/EldenRing/<steamid>/`. Returning OUR staged root here makes
/// the game build AND open the full save path under our tree NATIVELY (Wine does case-insensitive
/// resolution), so the character is read without depending on intercepting each handle-relative open.
pub(crate) use er_telemetry::counters::SAVE_REDIRECT_SHGFP_APPDATA_REQUESTS;
pub(crate) use er_telemetry::counters::SAVE_REDIRECT_SHGFP_DIRECT_FILE_BLOCKS;
pub(crate) use er_telemetry::counters::SAVE_REDIRECT_SHGFP_FIRST_LOAD_DONE_BLOCKS;
pub(crate) use er_telemetry::counters::SAVE_REDIRECT_SHGFP_LOGGED;
pub(crate) use er_telemetry::counters::SAVE_REDIRECT_SHGFP_NO_ROOT_BLOCKS;
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
pub(crate) use er_telemetry::counters::SAVE_NTCREATE_DIAG_LOGGED;
pub(super) const SAVE_NTCREATE_DIAG_MAX: usize = 120;
/// THE corruption fix (corrupted-save-re-findings): the save commit prechecks free space via
/// GetDiskFreeSpaceExW(saveDir), which on the Wine Z:->/home drive mapping returns bogus/ZERO free
/// space -> `free < needed` -> the write aborts BEFORE any byte ("Failed to save game / corrupted").
/// We hook it to report ample free space for the save dir so the game's OWN save flow writes our
/// staged save (no hardcoded paths, no Steam Cloud).
pub(crate) use er_telemetry::counters::SAVE_DISKFREE_LOGGED;
/// The game doesn't call kernel32!GetDiskFreeSpaceExW from our hook (no fire) -- under Wine all
/// free-space queries funnel to ntdll!NtQueryVolumeInformationFile. Override the AVAILABLE allocation
/// units for FileFsSizeInformation(3)/FileFsFullSizeInformation(7) so the save-commit free-space
/// precheck sees ample space regardless of the bogus Z:-drive report. THE corruption fix, robust.
pub(crate) use er_telemetry::counters::SAVE_VOLINFO_LOGGED;
/// One-shot/idempotency state for the core and redirect save-hook installers. The shared
/// `er-save-redirect` type owns this contract so later standalone hook-owner code does not invent a
/// second install state machine.
pub(super) static SAVE_HOOK_INSTALL_STATE: SaveHookInstallState = SaveHookInstallState::new();
/// Count of save-path opens we have redirected, logged for the first few so a probe can CONFIRM the
/// game actually opened our staged save through the redirect (not the default dir). Capped so a
/// busy IO loop cannot spam the debug log.
pub(crate) use er_telemetry::counters::SAVE_REDIRECT_HITS;
pub(crate) use er_telemetry::counters::SAVE_STEAM_API_STEAM_ID_LOGGED;
pub(crate) use er_telemetry::counters::SAVE_STEAM_ID_ENV_NORMALIZE_DONE;
const SAVE_REDIRECT_LOG_MAX: usize = 8;
/// Diagnostic: total CreateFileW calls our detour observed (proves the hook is live at all under
/// Wine's kernel32->kernelbase forwarding), and a bounded log of save-LIKE paths so we can see the
/// exact path form the game opens the save with (to fix the filter or confirm a missed hook).
pub(crate) use er_telemetry::counters::SAVE_CREATEFILEW_CALLS;
pub(crate) use er_telemetry::counters::SAVE_CREATEFILEW_DIAG_LOGGED;
const SAVE_CREATEFILEW_DIAG_MAX: usize = 200;
/// Sparse-sampling counter for the save-LIKE CreateFileW diag line (the `save_like` opens churn
/// thousands of identical lines per run). Logs the first 8 hits then only at power-of-two intervals
/// (16/32/64/...) -- same rate-limit pattern as `now_loading_helper_update_hook` -- so the diagnostic
/// keeps its early window and a sparse tail without flooding the debug log.
pub(crate) use er_telemetry::counters::SAVE_CREATEFILEW_DIAG_HITS;
static SAVE_CREATEFILEW_LAST_SAVE_LIKE_KIND: AtomicUsize =
    AtomicUsize::new(SavePathKind::None.as_usize());
pub(crate) use er_telemetry::counters::SAVE_CREATEFILEW_CONFIGURED_FILE_HITS;
pub(crate) use er_telemetry::counters::SAVE_CREATEFILEW_STAGE_SAVE_FILE_HITS;
pub(crate) use er_telemetry::counters::SAVE_CREATEFILEW_STAGE_STEAMID_DIR_HITS;
static MISSING_SAVE_DIALOG_GATE: er_save_redirect::MissingSaveGate =
    er_save_redirect::MissingSaveGate::new();
pub(crate) use er_telemetry::counters::MISSING_SAVE_BLOCKED_IO_LOGGED;
static SAVE_QUERY_LAST_SAVE_LIKE_KIND: AtomicUsize =
    AtomicUsize::new(SavePathKind::None.as_usize());

fn set_missing_save_dialog_state(state: er_save_redirect::MissingSaveState) {
    MISSING_SAVE_DIALOG_GATE.set(state);
}

pub(crate) fn missing_save_selection_pending() -> bool {
    MISSING_SAVE_DIALOG_GATE.is_pending()
}

/// True after an explicit loose save source (`er-effects.toml save_file` / ER_EFFECTS_SAVE_FILE) or
/// the in-game picker has activated direct-file staging. In this mode the user's original save is a
/// read-only source and native reads/writes target our private staged `%APPDATA%` tree. The load path
/// must use the full-read chain that reads the staged file directly instead of waiting on the native
/// Continue row/profile-summary path, which can be stale/empty for loose saves.
pub(crate) fn direct_save_file_source_active() -> bool {
    SAVE_DIRECT_SOURCE_FILE.get().is_some()
}
pub(crate) use er_telemetry::counters::SAVE_QUERY_CONFIGURED_FILE_HITS;
pub(crate) use er_telemetry::counters::SAVE_QUERY_STAGE_SAVE_FILE_HITS;
pub(crate) use er_telemetry::counters::SAVE_QUERY_STAGE_STEAMID_DIR_HITS;
/// DEDICATED budget for save-FILE queries (paths ending .sl2 / .co2 or containing ER0000): the shared
/// CreateFileW/existence-check diag cap above is exhausted by early-boot `eldenring\` dir churn
/// (GraphicsConfig.xml etc.) BEFORE the actual save read, hiding whether/with-what-steamid the game
/// ever queries ER0000.sl2. This separate counter guarantees those queries are always logged. Reveals
/// the exact `EldenRing\<steamid>\ER0000.sl2` path the game builds (steamid match vs the staged 766).
pub(crate) use er_telemetry::counters::SAVE_SL2_QUERY_LOGGED;
const SAVE_SL2_QUERY_MAX: usize = 40;

/// Frames of "profile summary present but ZERO active slots" tolerated before the save-load watchdog
/// aborts. ~15s at 60fps -- long enough to ignore the boot transient before the summary is parsed,
/// short enough to fast-fail well under the runtime cap instead of stalling on the privacy policy.
pub(crate) use er_telemetry::counters::SAVE_WATCHDOG_ZERO_FRAMES;
pub(crate) const SAVE_WATCHDOG_ZERO_BUDGET: usize = 900;

/// Resolve configured save file -> the staged save ROOT (the ancestor directory that CONTAINS the
/// `EldenRing` folder) in Wine `Z:\...` wide form, or None if config/env is unset/blank/not a readable
/// plausibly-sized save / not staged under an `EldenRing` directory component. The redirect rewrites
/// the game's `...\Roaming\EldenRing\<rest>` to `<root>\EldenRing\<rest>`, so the staged save MUST
/// live at `<root>/EldenRing/<steamid>/ER0000.sl2`.
fn env_save_file_path() -> Option<PathBuf> {
    configured_save_file()
}

type SaveRedirectSource = er_save_redirect::SaveSourcePlan;

fn validated_save_file_path(path: PathBuf) -> Option<PathBuf> {
    match er_save_redirect::validate_save_file_path(path) {
        Ok(path) => Some(path),
        Err(err) => {
            append_autoload_debug(format_args!(
                "save-override: rejected save source during shared validation: {err:?}"
            ));
            None
        }
    }
}

fn picker_status_for_save_source_rejection(
    err: er_save_redirect::SaveSourceRejection,
) -> er_save_picker::PickerStatusMessage {
    match err {
        er_save_redirect::SaveSourceRejection::MissingOrNotFile => {
            er_save_picker::PickerStatusMessage::new(
                "SAVE NOT FOUND",
                "The selected path is missing or is not a file.",
            )
        }
        er_save_redirect::SaveSourceRejection::WrongSize { len, expected } => {
            er_save_picker::PickerStatusMessage::new(
                "WRONG SAVE SIZE",
                format!("Expected {expected} bytes, but this file is {len} bytes."),
            )
        }
        er_save_redirect::SaveSourceRejection::NotBnd4 => er_save_picker::PickerStatusMessage::new(
            "NOT AN ELDEN RING SAVE",
            "The file is not a readable BND4 save container.",
        ),
        er_save_redirect::SaveSourceRejection::Unreadable => {
            er_save_picker::PickerStatusMessage::new(
                "SAVE UNREADABLE",
                "The save exists, but could not be read.",
            )
        }
    }
}

/// Read-only is NEVER a reason to refuse a save, at any surface. Loading is a pure READ and succeeds
/// on a `0444` file; the bit can only bite later, at the first WRITE.
///
/// For a PICKED or CONFIGURED save it cannot bite at all: those are staged into a private native tree
/// and `save_redirect_target_for_path` sends reads *and writes* to the staged copy, so the source is
/// never a write target (the repo corpus is `0444` and loads fine). Those paths call
/// `validated_save_file_path` directly and never reach this function.
///
/// For the DEFAULT save the bit is worth REPAIRING, because `DEFAULT-USER-SAVE` mode writes the active
/// Steam user's APPDATA save in place, and Elden Ring owns that file and requires it writable -- a
/// stray read-only bit (botched copy/restore, bd `er-save-files-readonly-staging-2026-06-26`) makes the
/// title-flow "Updating save data" write fail with "Failed to save game. Save data is corrupted.". So
/// clear it when we can, exactly as staging does for its own copies.
///
/// When we CANNOT clear it, still load. Refusing repairs nothing: the APPDATA save stays unwritable
/// either way, and the only alternative on offer -- bouncing to the picker -- is strictly worse,
/// because any save picked there is STAGED, so the user's progress would silently land in the staged
/// tree instead of the save they meant to play. The game's own save-failure popup, on the save they
/// actually asked for, beats a silent substitution.
fn validated_default_save_file(path: PathBuf, source_label: &str) -> Option<PathBuf> {
    let validated = validated_save_file_path(path)?;
    if save_file_is_readonly(&validated) {
        make_file_writable(&validated);
        if save_file_is_readonly(&validated) {
            append_autoload_debug(format_args!(
                "save-override: DEFAULT save '{}' source={source_label} is read-only and could NOT be made writable -- LOADING IT ANYWAY (the load is a pure read); expect the game's own save-failure popup at the first autosave/quit-save until the file is made writable",
                validated.display()
            ));
        } else {
            append_autoload_debug(format_args!(
                "save-override: cleared a stray read-only bit on the game-owned DEFAULT save '{}' source={source_label} (Elden Ring writes this file in place and requires it writable)",
                validated.display()
            ));
        }
    }
    Some(validated)
}

fn validated_configured_save_file() -> Option<PathBuf> {
    // Staged, never written in place -- read-only is valid, so no writability check here.
    validated_save_file_path(env_save_file_path()?)
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

/// The GAME-SIDE save directory whose write-opens this crate's general save redirect maps into
/// the staged tree, or `None` when no general redirect is installed (the default game-owned
/// APPDATA mode, where the game already opens the live save directly).
///
/// The save-destination commit needs this to match write-opens by their FULL path. In staged /
/// direct-file mode the loaded save lives under the staged root, but the native writer still
/// opens `...\Roaming\EldenRing\<steamid>\ER0000.sl2`, and that open IS the loaded save's --
/// `save_redirect_path` rewrites it a moment later. Matching only the leaf caught it by
/// accident; matching the full path has to know about the mapping.
pub(crate) fn save_redirect_native_source_dir() -> Option<PathBuf> {
    SAVE_REDIRECT_DIR_W.get()?;
    let steam_id = plausible_steam_id64(OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst))?;
    Some(default_save_root()?.join(steam_id.to_string()))
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

/// The container name the active runtime WRITES to. Seamless Co-op (ERSC) keeps co-op progress in
/// `ER0000.co2` -- a separate container from the vanilla `ER0000.sl2`. This is the single native
/// write target: it names the staged copy and the `%APPDATA%` path our hook redirects, so the
/// container that gets loaded and the container that gets written are always the same file.
///
/// The Seamless co-op container ERSC is CONFIGURED with, read from its own `ersc_settings.ini`.
///
/// `ER0000.co2` is ERSC's shipped default, NOT a fixed name: the ini says "Your save file extension
/// (in the vanilla game this is .sl2). Use any alphanumeric characters (limit = 120)". A user who
/// changes it gets a differently-named container, and every hard-coded `.co2` in a save path would
/// then name a file the runtime never opens -- the same failure as staging under the wrong name.
///
/// Resolved ONCE and cached, because unlike the ERSC module latch this answer is available
/// immediately: the ini is on disk before the process starts, so staging can use it at
/// DllMain+~190ms while `seamless_coop_loaded()` is still false. The settings file sits beside
/// `ersc.dll`; when that module is not resident yet we fall back to the game's own
/// `SeamlessCoop/` directory, then to ERSC's default extension.
pub(crate) fn seamless_save_container_name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let (extension, source) = ersc_settings_path()
            .and_then(|path| {
                let ini = fs::read_to_string(&path).ok()?;
                let extension = er_save_redirect::parse_ersc_save_file_extension(&ini)?.to_owned();
                Some((extension, path.display().to_string()))
            })
            .unwrap_or_else(|| {
                (
                    er_save_redirect::DEFAULT_SEAMLESS_SAVE_FILE_EXTENSION.to_owned(),
                    "ersc default (no readable ersc_settings.ini)".to_owned(),
                )
            });
        let name = er_save_redirect::save_container_name_for_extension(&extension);
        append_autoload_debug(format_args!(
            "save-override: Seamless save container resolved to '{name}' from {source}"
        ));
        Box::leak(name.into_boxed_str())
    })
}

/// On-disk path of an already-loaded module, or None when it is not resident.
fn resident_module_path(module_name: &[u8]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    let module = unsafe { GetModuleHandleA(PCSTR::from_raw(module_name.as_ptr())) }.ok()?;
    if module.0 as usize == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let len = unsafe {
        crate::ffi::GetModuleFileNameW(module.0 as isize, buf.as_mut_ptr(), buf.len() as u32)
    };
    if len == 0 || len as usize >= buf.len() {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_wide(
        &buf[..len as usize],
    )))
}

/// `ersc_settings.ini` beside the resident `ersc.dll`, else under the game's `SeamlessCoop/`.
fn ersc_settings_path() -> Option<PathBuf> {
    const SETTINGS_FILE: &str = "ersc_settings.ini";
    let beside_module = resident_module_path(b"ersc.dll\0")
        .and_then(|path| path.parent().map(|dir| dir.join(SETTINGS_FILE)))
        .filter(|path| path.is_file());
    beside_module.or_else(|| {
        let path = game_directory_path()?
            .join("SeamlessCoop")
            .join(SETTINGS_FILE);
        path.is_file().then_some(path)
    })
}

/// This is NOT the acceptance predicate -- see [`active_default_save_file_names`] -- and it is NOT
/// the staging name either. Staging cannot use it: it runs at DllMain+~190ms, before me3 has loaded
/// `ersc.dll`, so the ERSC latch below still answers "vanilla" on a Seamless launch. Staging writes
/// every container name instead (`staged_save_container_names`); this one names the WRITE target,
/// which is resolved later, once the latch has settled.
pub(crate) fn active_default_save_file_name() -> &'static str {
    if save_picker_seamless_mode_after_settle("active-default-save-file-name") {
        seamless_save_container_name()
    } else {
        er_save_redirect::VANILLA_SAVE_CONTAINER_NAME
    }
}

/// Every container name a staging pass writes, cached for the process.
pub(crate) fn staged_save_container_names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        er_save_redirect::staged_save_container_names_for(seamless_save_container_name())
            .into_iter()
            .map(|name| &*Box::leak(name.into_boxed_str()))
            .collect()
    })
}

/// Container names the active runtime will LOAD, in priority order.
///
/// The mode lock is deliberately ASYMMETRIC (user spec 2026-08-02), matching what every picker
/// surface already offers (`save_picker_boot.rs`, `save_picker_menu.rs`,
/// `system_quit_dialog_handlers.rs` all use `if seamless { ["co2","sl2"] } else { ["sl2"] }`):
///
/// * **Seamless** takes BOTH the co-op container and `ER0000.sl2`, preferring the co-op one when
///   both exist. Refusing a vanilla `.sl2` here is what softlocked the loading screen on
///   2026-08-02 (bd `er-effects-rs-h6sh`): the picker legitimately accepted one and nothing
///   downstream would load it. The co-op container is whatever ERSC is CONFIGURED with -- see
///   [`seamless_save_container_name`] -- not a fixed `ER0000.co2`.
/// * **Vanilla** takes ONLY `ER0000.sl2`. A vanilla launch must never silently load the Seamless
///   container, because that would advance co-op progress in an offline session.
///
/// If none of these is present, default-save discovery returns "no save" and the missing-save
/// picker asks the user for a file.
pub(crate) fn active_default_save_file_names() -> &'static [&'static str] {
    static VANILLA_ONLY: [&str; 1] = [er_save_redirect::VANILLA_SAVE_CONTAINER_NAME];
    static SEAMLESS: OnceLock<Vec<&'static str>> = OnceLock::new();
    if !save_picker_seamless_mode_after_settle("active-default-save-file-names") {
        return &VANILLA_ONLY;
    }
    SEAMLESS.get_or_init(|| {
        er_save_redirect::active_save_container_names_for(true, seamless_save_container_name())
            .into_iter()
            .map(|name| &*Box::leak(name.into_boxed_str()))
            .collect()
    })
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
    let root = default_save_root()?;
    // Asymmetric mode lock: Seamless takes `.co2` then falls back to `.sl2`; vanilla takes only
    // `.sl2`. First candidate that holds a readable character wins.
    active_default_save_file_names().iter().find_map(|name| {
        validated_default_save_file(
            default_save_file_path(&root, steam_id, name),
            "active-default-save",
        )
        .and_then(default_save_with_character)
    })
}

fn default_save_file_candidates() -> Vec<(PathBuf, u64)> {
    let Some(root) = default_save_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let steam_id = entry
                .file_name()
                .to_str()
                .and_then(steam_id64_from_dir_name)?;
            active_default_save_file_names()
                .iter()
                .find_map(|name| {
                    validated_default_save_file(
                        default_save_file_path(&root, steam_id, name),
                        "default-save-candidate",
                    )
                    .and_then(default_save_with_character)
                })
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
    Some(default_save_file_path(
        &default_save_root()?,
        steam_id,
        active_default_save_file_name(),
    ))
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

fn save_redirect_source_for_validated_file(path: PathBuf) -> SaveRedirectSource {
    // Explicit/user-picked non-default saves are read-only sources, even if they already live under an
    // `EldenRing/<steamid>/ER0000.*` layout. The game writes only to our private staged copy unless
    // the shared planner proves this is the current game-owned default-save root and writeback is safe.
    let writeback_allowed = save_file_writeback_allowed(&path);
    er_save_redirect::plan_validated_save_source(path, writeback_allowed)
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
            root_wide,
        } => {
            OBSERVED_ACTIVE_STEAM_ID64.store(steam_id, Ordering::SeqCst);
            normalize_env_save_file_to_known_steam_id(&file, steam_id, source_label);
            SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(1, Ordering::SeqCst);
            // UTF-8 Lossy: log-only decode of configured Windows wide path for probe confirmation.
            let shown = String::from_utf16_lossy(root_wide.as_slice());
            let _ = SAVE_REDIRECT_DIR_W.set(root_wide.into_vec());
            SAVE_REDIRECT_MODE.store(SAVE_REDIRECT_MODE_STAGED_ROOT, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-override: ENFORCED -- redirecting native save root to staged root '{shown}' source={source_label}"
            ));
            SaveOverrideMode::Redirect
        }
        SaveRedirectSource::DirectFile {
            file,
            stage_root,
            root_wide,
        } => {
            for dir in direct_stage_case_dirs(&stage_root) {
                let _ = std::fs::create_dir_all(dir);
            }
            // UTF-8 Lossy: log-only decode of configured source/stage paths for probe confirmation.
            let shown = file.display().to_string();
            let stage_shown = String::from_utf16_lossy(root_wide.as_slice());
            let configured_file = file.clone();
            let explicit_steam_id = configured_active_steam_id64();
            let _ = SAVE_DIRECT_SOURCE_FILE.set(file);
            let _ = SAVE_DIRECT_STAGE_ROOT.set(stage_root);
            let _ = SAVE_REDIRECT_DIR_W.set(root_wide.into_vec());
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
        "save-override: no usable autoload save (configured save missing/invalid, or no readable active default among {:?} exactly {} bytes; read-only is NOT a rejection reason). config_error={}. Arming the IN-GAME missing-save picker: the title boots to its native no-save menu and the 05_010 file browser presents itself (save_picker_menu.rs); world entry stays denied until a save is picked.",
        active_default_save_file_names(),
        SAVE_OVERRIDE_EXPECTED_BYTES,
        runtime_config_error().unwrap_or_else(|| "none".to_owned())
    ));
    set_missing_save_dialog_state(er_save_redirect::MissingSaveState::Pending);
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
/// the missing-save gate. A rejection carries the user-facing reason and leaves state unchanged so
/// the picker stays up.
pub(crate) fn complete_missing_save_selection_from_picker(
    path: &Path,
) -> er_save_picker::MissingSaveSelectionOutcome {
    use er_save_picker::MissingSaveSelectionOutcome;
    // NO writability check: the pick is staged into a private native tree and the source is never a
    // write target, so a read-only save (the norm for the `0444` repo corpus) loads exactly like a
    // writable one. Rejecting those was a false negative that looked to the user like the picker
    // simply refusing to open the save.
    let validated = match er_save_redirect::validate_save_file_path(path.to_path_buf()) {
        Ok(path) => path,
        Err(err) => {
            let message = picker_status_for_save_source_rejection(err);
            append_autoload_debug(format_args!(
                "save-override: title picker rejected invalid save '{}' -- {err:?} visible='{}: {}'",
                path.display(),
                message.headline(),
                message.detail()
            ));
            return MissingSaveSelectionOutcome::Rejected(message);
        }
    };
    match fs::read(&validated) {
        Ok(bytes) if er_save_loader::bnd4::parse_entries(&bytes).is_ok() => {}
        Ok(bytes) => {
            let message = er_save_picker::PickerStatusMessage::new(
                "NOT AN ELDEN RING SAVE",
                "The file is not a readable BND4 save container.",
            );
            append_autoload_debug(format_args!(
                "save-override: title picker rejected non-BND4 file '{}' len={} visible='{}: {}'",
                validated.display(),
                bytes.len(),
                message.headline(),
                message.detail()
            ));
            return MissingSaveSelectionOutcome::Rejected(message);
        }
        Err(err) => {
            let message = er_save_picker::PickerStatusMessage::new(
                "SAVE UNREADABLE",
                "The save exists, but could not be read.",
            );
            append_autoload_debug(format_args!(
                "save-override: title picker could not read '{}': {err} visible='{}: {}'",
                validated.display(),
                message.headline(),
                message.detail()
            ));
            return MissingSaveSelectionOutcome::Rejected(message);
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
    set_missing_save_dialog_state(er_save_redirect::MissingSaveState::Ready);
    append_autoload_debug(format_args!(
        "save-override: title picker selected save '{}'; redirect active, missing-save gate released",
        validated.display()
    ));
    MissingSaveSelectionOutcome::Completed
}

/// Diagnostic-only observer for save-like IO while the missing-save selection is pending. The
/// IN-GAME picker flow REQUIRES this IO to proceed: the title must complete its natural no-save
/// boot (empty ProfileSummary, interactive menu) for the 05_010 file browser to present itself --
/// blocking here re-creates the input-dead title the old OS dialog existed to paper over. The
/// pick later installs/activates the redirect and fires a title reload, so nothing read during
/// the pending window is ever committed.
pub(super) fn wait_for_missing_save_dialog_if_pending(path: &[u16]) {
    if !MISSING_SAVE_DIALOG_GATE.is_pending() {
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

/// Length of a NUL-terminated UTF-16 string at `ptr` (excludes the NUL). 0 on null pointer.
pub(super) unsafe fn wide_len(ptr: *const u16) -> usize {
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

/// Index just after the last path separator in `path` (0 if none) -- the basename start.
fn ensure_direct_stage_for_requested_path(path: &[u16]) {
    let Some(root) = SAVE_DIRECT_STAGE_ROOT.get() else {
        return;
    };
    match plan_direct_stage_request(path) {
        DirectStageRequestPlan::SteamId(steam_id) => ensure_direct_stage_for_steam_id(steam_id),
        DirectStageRequestPlan::NoSteamId(kind) => {
            SAVE_DIRECT_STAGE_NO_STEAMID_HITS.fetch_add(1, Ordering::SeqCst);
            SAVE_DIRECT_STAGE_LAST_NO_STEAMID_KIND.store(kind.as_usize(), Ordering::SeqCst);
            let hit = SAVE_DIRECT_STAGE_DIAG_HITS.fetch_add(1, Ordering::SeqCst);
            if hit < 8 {
                // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
                let shown = String::from_utf16_lossy(path);
                let kind_label = kind.label();
                append_autoload_debug(format_args!(
                    "save-override: direct-file stage pending -- no SteamID64 in {kind_label} requested path '{shown}'"
                ));
            }
            for dir in direct_stage_case_dirs(root) {
                let _ = std::fs::create_dir_all(dir);
            }
        }
    }
}

fn make_file_writable(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            // Windows-only DLL: this clears FILE_ATTRIBUTE_READONLY, the one bit we mean. The
            // lint is about the UNIX behaviour (0o666 for everyone), which cannot occur here.
            #[allow(clippy::permissions_set_readonly_false)]
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

/// Read the configured source once and stamp the active account's SteamID into it.
///
/// One read for the whole staging pass: the same bytes go to every container name, and re-reading
/// and re-normalizing a 28 MB save per target would multiply the DllMain staging cost by the size
/// of the name set.
fn read_normalized_save_for_stage(source: &Path, steam_id: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = std::fs::read(source)?;
    match er_save_loader::bnd4::normalize_steam_id_in_place(&mut bytes, steam_id) {
        Ok(report) if report.changed() => append_autoload_debug(format_args!(
            "save-override: direct-file staging normalized private copy source='{}' steam_id={steam_id} char_patched={} user_data10_patched={} md5_rewritten={}",
            source.display(),
            report.character_slots_patched,
            report.user_data10_patched,
            report.md5_rewritten
        )),
        Ok(_) => {}
        Err(err) => append_autoload_debug(format_args!(
            "save-override: direct-file staging normalization skipped source='{}' steam_id={steam_id}: {err:?}",
            source.display()
        )),
    }
    Ok(bytes)
}

fn write_staged_save(bytes: &[u8], target: &Path) -> std::io::Result<u64> {
    remove_file_for_overwrite(target)?;
    std::fs::write(target, bytes)?;
    make_file_writable(target);
    Ok(bytes.len() as u64)
}

fn ensure_direct_stage_for_steam_id(steam_id: u64) {
    // Staging is a read of the source plus two writes of the staged copies -- three opens that
    // re-enter whichever detour asked for the staging. The `SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID`
    // latch below happens to swallow a re-entry for the SAME id, but not for a different one, and
    // relying on that accident is how this file grew a stack overflow. Refuse outright from a
    // nested context: the outer detour entry stages it once its own I/O has unwound.
    if !save_detour_disk_io_allowed() {
        return;
    }
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
    // Stage the source under EVERY container name (`STAGED_SAVE_CONTAINER_NAMES`), never under a
    // name derived from the source's own extension and never under one derived from the Seamless
    // mode. This code runs inside the `CreateFileW` detour at DllMain+~190ms, and me3 loads
    // `ersc.dll` AFTER that, so `active_default_save_file_name()` here still answers "vanilla" on a
    // Seamless launch -- measured 2026-08-11, the same run that logged
    // `seamless=false reason=active-default-save-file-name` at +191ms reported
    // `seamless_coop_loaded=true` in its telemetry. Naming the staged copy from that unsettled
    // latch put the save at `ER0000.sl2` while `own_load::drive` and the native writer later asked
    // for `ER0000.co2`; the two never met and the boot cover parked forever.
    //
    // Restamping the name is byte-safe: `.sl2` and `.co2` are the same 28 MB BND4 container and
    // the copy below rewrites the embedded Steam ID either way. `active_default_save_file_name()`
    // keeps its job of naming the WRITE target, which is resolved later, once the latch has
    // settled.
    let stage_dirs: Vec<PathBuf> = direct_stage_case_dirs(root)
        .into_iter()
        .map(|dir| dir.join(steam_id.to_string()))
        .collect();
    for dir in &stage_dirs {
        if let Err(err) = std::fs::create_dir_all(dir) {
            append_autoload_debug(format_args!(
                "save-override: direct-file stage failed creating '{}': {err}",
                dir.display()
            ));
            SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.store(0, Ordering::SeqCst);
            return;
        }
    }
    // Under Wine the two case spellings resolve to ONE directory, so writing each 28 MB container
    // once per spelling would double the DllMain staging cost for no extra coverage.
    let stage_dirs = dedupe_dirs_by_identity(stage_dirs, |dir| std::fs::canonicalize(dir).ok());

    let bytes = match read_normalized_save_for_stage(source, steam_id) {
        Ok(bytes) => bytes,
        Err(err) => {
            append_autoload_debug(format_args!(
                "save-override: direct-file stage could NOT read the configured source '{}' for SteamID64 {steam_id}: {err} -- nothing was staged; the runtime will open whatever is already in the staged tree",
                source.display()
            ));
            SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.store(0, Ordering::SeqCst);
            return;
        }
    };
    let container_names = staged_save_container_names();
    let mut staged: Vec<PathBuf> = Vec::new();
    for dir in &stage_dirs {
        for name in container_names {
            let target = dir.join(name);
            match write_staged_save(&bytes, &target) {
                Ok(bytes) => {
                    SAVE_DIRECT_STAGE_CONTAINERS_WRITTEN.fetch_add(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "save-override: direct-file staged {bytes} bytes for SteamID64 {steam_id}: '{}' -> '{}'",
                        source.display(),
                        target.display()
                    ));
                    staged.push(target);
                }
                Err(err) => {
                    // LOUD: a container the runtime may open does NOT hold the configured source.
                    // Leaving the done-latch unset lets a later save-path observation retry.
                    append_autoload_debug(format_args!(
                        "save-override: direct-file stage copy FAILED for SteamID64 {steam_id}: '{}' -> '{}': {err} -- the runtime may open a container that is NOT the configured save",
                        source.display(),
                        target.display()
                    ));
                    SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.store(0, Ordering::SeqCst);
                    return;
                }
            }
        }
    }
    for dir in &stage_dirs {
        remove_stale_staged_saves(dir, source, container_names);
    }
    SAVE_DIRECT_STAGE_DONE_STEAM_ID.store(steam_id, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-override: direct-file staging DONE for SteamID64 {steam_id}: '{}' now backs {} staged container(s) {:?}; stale_removed={} stale_remove_failed={}",
        source.display(),
        staged.len(),
        staged
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        SAVE_DIRECT_STAGE_STALE_REMOVED.load(Ordering::SeqCst),
        SAVE_DIRECT_STAGE_STALE_REMOVE_FAILED.load(Ordering::SeqCst)
    ));
    SAVE_DIRECT_STAGE_IN_PROGRESS_STEAM_ID.store(0, Ordering::SeqCst);
}

/// Delete every save artifact in a staged SteamID directory that this pass did not just write.
///
/// A staged `.co2` from an earlier run, or a `.bak` companion the game left behind, is a save the
/// user did not configure -- and the loader prefers whichever container the active mode names, not
/// whichever is newest. Measured 2026-08-11: a `100-Lilbro` run was served an `ER0000.co2` written
/// 33 minutes earlier from a DIFFERENT source ('angrE' level 120 vs the configured level 100), the
/// slot read back blank, and the autoload guard refused to continue -- a silent soft lock at the
/// boot cover with nothing in the log naming the file actually loaded.
///
/// Only the private stage tree is ever touched: the configured source lives one directory ABOVE
/// the stage root and is read-only by contract, and `is_inside_direct_stage_root` fails the whole
/// sweep closed if a caller ever hands over a directory outside it.
fn remove_stale_staged_saves(dir: &Path, source: &Path, container_names: &[&str]) {
    if !is_inside_direct_stage_root(dir) {
        append_autoload_debug(format_args!(
            "save-override: REFUSING to sweep stale saves in '{}' -- not inside a '{DIRECT_STAGE_ROOT_DIR_NAME}' tree",
            dir.display()
        ));
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        // UTF-8 Lossy: staged file-name classification only; a name whose host bytes are not valid
        // UTF-8 was not written by staging and is classified from its lossy form deterministically.
        let name = entry.file_name().to_string_lossy().into_owned();
        if staged_entry_fate(&name, container_names) != StagedEntryFate::StaleRemove {
            continue;
        }
        let path = entry.path();
        match remove_file_for_overwrite(&path) {
            Ok(()) => {
                SAVE_DIRECT_STAGE_STALE_REMOVED.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-override: removed STALE staged save '{}' -- it is a leftover from an earlier run, not the configured source '{}'",
                    path.display(),
                    source.display()
                ));
            }
            Err(err) => {
                // LOUD: the leftover is still on disk and the game can still open it.
                SAVE_DIRECT_STAGE_STALE_REMOVE_FAILED.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-override: FAILED to remove STALE staged save '{}': {err} -- the game may load THIS file instead of the configured source '{}'",
                    path.display(),
                    source.display()
                ));
            }
        }
    }
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
    // Direct-file mode stages the selected source into the private native save tree. Do NOT redirect
    // save-file or .bak opens to `SAVE_DIRECT_SOURCE_FILE`; reads and writes must hit the staged copy
    // so readonly/user-provided source saves are never modified by gameplay or profile switching.
    redirect_wide_save_path_with_side_effects(
        path,
        SAVE_REDIRECT_DIR_W.get().map(Vec::as_slice),
        observe_steam_id64_from_save_path,
        ensure_direct_stage_for_requested_path,
    )
}

type CreateFileWFn =
    unsafe extern "system" fn(*const u16, u32, u32, *const c_void, u32, u32, isize) -> isize;
type CopyFileWFn = unsafe extern "system" fn(*const u16, *const u16, i32) -> i32;

/// CreateFileW detour: redirect save-file opens to the env dir; pass everything else through.
/// Covers BOTH read and write (the returned HANDLE is reused by ReadFile/WriteFile).
pub(super) unsafe extern "system" fn save_redirect_createfilew_hook(
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
    // RE-ENTRANCY (see `reentry.rs`): this detour body does its own file I/O -- the SteamID
    // normalize's `fs::read`, direct-file staging's read+writes, the debug log's own open -- and
    // every one of those comes back through `CreateFileW`, i.e. right back here on this thread.
    // A nested entry is OUR open of a path WE computed, so it wants the original API untouched and
    // none of the observation/redirect/staging/diagnostic work. Doing that work again is what
    // recursed 510 frames deep into the guard page on 2026-07-30.
    // `SAVE_CREATEFILEW_CALLS` counts every entry, nested ones included: its job is to prove the
    // hook is live at all, and a run whose nested count runs away is itself evidence.
    let calls = SAVE_CREATEFILEW_CALLS.fetch_add(1, Ordering::SeqCst);
    let depth = SaveDetourDepth::enter();
    if depth.is_reentrant() {
        return unsafe {
            call(
                lp_file_name,
                access,
                share,
                security,
                disposition,
                flags,
                template,
            )
        };
    }
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        // SAVE-DESTINATION WRITE-OPEN REDIRECT (save-game-flow WP3), checked FIRST and only inside
        // the armed commit window: the native BND4 writer emits the whole rebuilt container in one
        // write-open, so diverting exactly that open writes the current save state to the folder
        // the user chose while the loaded save is never opened for write. Read-opens (the writer's
        // own read-modify-write base) and the `.bak` CopyFileW deliberately pass through.
        if let Some(destination) = save_dest_redirect_for_open(path, access) {
            let ret = unsafe {
                call(
                    destination.as_ptr(),
                    access,
                    share,
                    security,
                    disposition,
                    flags,
                    template,
                )
            };
            save_dest_note_redirect_hit(ret != -1);
            return ret;
        }
        // Observe/stage before any redirect decision. Direct-file mode needs the native path builder's
        // `<steamid>` component to populate the private discovery tree, and some paths are diagnostic
        // only (not redirected) but still carry the account id.
        observe_steam_id64_from_save_path(path);
        // Diagnostic: confirm the hook is live (log the very first call), then log save-LIKE paths
        // (contain "eldenring" or end .sl2/.co2/.bak) so we can see the exact save path form even when
        // the redirect filter does NOT match -- distinguishes "hook never fires" from "filter misses".
        let plan = plan_create_file_open(path, save_redirect_path);
        let diag = plan.diag;
        if diag.save_like {
            record_save_like_createfile_path_kind(path);
            // ATTRIBUTION, not a gate. A modal OS file dialog enumerates folders on THIS thread, so
            // its shell traffic re-enters this detour and any path containing "eldenring" or ending
            // .sl2/.co2/.bak counts as save-like -- polluting the save CreateFileW diagnostics with
            // browsing. Counting the ones seen while a dialog was open makes that noise
            // attributable instead of indistinguishable from the game's own save I/O. These are
            // READ opens that pass through unredirected; the shell does not write the loaded save,
            // so this is a reporting concern, not a corruption one -- but it must be visible.
            if er_telemetry::counters::SAVE_PICKER_OS_DIALOG_OPEN.load(Ordering::SeqCst) != 0 {
                er_telemetry::counters::SAVE_PICKER_OS_SAVELIKE_OPENS
                    .fetch_add(1, Ordering::SeqCst);
            }
        }
        if diag.should_capture_diag_log(calls) {
            // Rate-limit: log the first 8 save-LIKE opens, then only at power-of-two hit counts.
            let hits = SAVE_CREATEFILEW_DIAG_HITS.fetch_add(1, Ordering::SeqCst) + 1;
            if createfile_diag_hit_should_log(hits) {
                // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
                let p = String::from_utf16_lossy(path);
                append_autoload_debug(format_args!(
                    "save-override: CreateFileW diag call#{calls} save_like={} diag_hits={hits} '{p}'",
                    diag.save_like
                ));
            }
        }
        if plan.should_wait_for_missing_save_dialog() {
            wait_for_missing_save_dialog_if_pending(path);
        }
        if plan.should_normalize_on_save_open()
            && let Ok(base) = game_module_base()
        {
            normalize_env_save_file_to_active_steam_id_once(base, "createfile-save-open");
        }
        if let Some(redirected) = plan.redirected {
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
pub(super) unsafe extern "system" fn save_redirect_copyfilew_hook(
    existing: *const u16,
    new_file: *const u16,
    fail_if_exists: i32,
) -> i32 {
    let orig = SAVE_REDIRECT_ORIG_COPYFILEW.load(Ordering::SeqCst);
    let call: CopyFileWFn = unsafe { std::mem::transmute::<usize, CopyFileWFn>(orig) };
    // RE-ENTRANCY (see `reentry.rs`): `save_redirect_path` observes, stages and normalizes, all of
    // which open files. Nested entries pass straight through with the caller's own endpoints.
    let depth = SaveDetourDepth::enter();
    if depth.is_reentrant() {
        return unsafe { call(existing, new_file, fail_if_exists) };
    }
    let existing_red = {
        let len = unsafe { wide_len(existing) };
        (len != 0)
            .then(|| unsafe { std::slice::from_raw_parts(existing, len) })
            .and_then(|path| {
                let plan = classify_copyfile_endpoint(path, save_redirect_path);
                if plan.should_wait_for_missing_save_dialog {
                    wait_for_missing_save_dialog_if_pending(path);
                }
                plan.redirected
            })
    };
    let new_red = {
        let len = unsafe { wide_len(new_file) };
        (len != 0)
            .then(|| unsafe { std::slice::from_raw_parts(new_file, len) })
            .and_then(|path| {
                let plan = classify_copyfile_endpoint(path, save_redirect_path);
                if plan.should_wait_for_missing_save_dialog {
                    wait_for_missing_save_dialog_if_pending(path);
                }
                plan.redirected
            })
    };
    let existing_ptr = existing_red.as_ref().map_or(existing, |v| v.as_ptr());
    let new_ptr = new_red.as_ref().map_or(new_file, |v| v.as_ptr());
    unsafe { call(existing_ptr, new_ptr, fail_if_exists) }
}

/// Shared diag + redirect decision for a save-existence-check API taking a wide path arg1. Logs
/// "eldenring"-containing paths (capped, shared budget) so we see the exact existence-check path
/// form, and returns the redirected NUL-terminated path when the save filter matches (else None).
fn save_path_api_redirect(api: &str, path: &[u16]) -> Option<Vec<u16>> {
    let plan = plan_save_query_path(path, save_redirect_path);
    let diag = plan.diag;
    // DEDICATED save-FILE query log (own budget; immune to the early-boot churn that exhausts the
    // shared cap below) -- captures the exact ER0000.sl2 existence/enum path + its <steamid> component.
    if diag.should_record_path_kind() {
        record_save_like_query_path_kind(path);
    }
    if diag.should_capture_save_file_query_log(
        SAVE_SL2_QUERY_LOGGED.load(Ordering::SeqCst),
        SAVE_SL2_QUERY_MAX,
    ) {
        let d = SAVE_SL2_QUERY_LOGGED.fetch_add(1, Ordering::SeqCst);
        if d < SAVE_SL2_QUERY_MAX {
            // UTF-8 Lossy: log-only decode of the save-file query path for probe diagnosis.
            let p = String::from_utf16_lossy(path);
            let did = if plan.redirected.is_some() {
                "REDIRECT"
            } else {
                "pass"
            };
            append_autoload_debug(format_args!("save-override: {api} SL2-QUERY {did} '{p}'"));
        }
    }
    if diag.should_capture_general_query_log(
        SAVE_CREATEFILEW_DIAG_LOGGED.load(Ordering::SeqCst),
        SAVE_CREATEFILEW_DIAG_MAX,
    ) {
        let d = SAVE_CREATEFILEW_DIAG_LOGGED.load(Ordering::SeqCst);
        if d < SAVE_CREATEFILEW_DIAG_MAX {
            SAVE_CREATEFILEW_DIAG_LOGGED.store(d + 1, Ordering::SeqCst);
            // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
            let p = String::from_utf16_lossy(path);
            let did = if plan.redirected.is_some() {
                "REDIRECT"
            } else {
                "pass"
            };
            append_autoload_debug(format_args!("save-override: {api} diag {did} '{p}'"));
        }
    }
    plan.redirected
}

/// GetFileAttributesW detour: redirect save-path existence checks to the env dir.
pub(super) unsafe extern "system" fn save_redirect_getattrw_hook(lp_file_name: *const u16) -> u32 {
    let orig = SAVE_REDIRECT_ORIG_GETATTRW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16) -> u32 =
        unsafe { std::mem::transmute::<usize, unsafe extern "system" fn(*const u16) -> u32>(orig) };
    // RE-ENTRANCY (see `reentry.rs`): `save_path_api_redirect` -> `save_redirect_path` observes,
    // stages and normalizes, all of which open files and land back in this detour family.
    let depth = SaveDetourDepth::enter();
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 && !depth.is_reentrant() {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("GetFileAttributesW", path) {
            return unsafe { call(red.as_ptr()) };
        }
    }
    unsafe { call(lp_file_name) }
}

/// GetFileAttributesExW detour: same redirect for the Ex existence check.
pub(super) unsafe extern "system" fn save_redirect_getattrexw_hook(
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
    // RE-ENTRANCY (see `reentry.rs`): same hazard as `GetFileAttributesW` above.
    let depth = SaveDetourDepth::enter();
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 && !depth.is_reentrant() {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("GetFileAttributesExW", path) {
            return unsafe { call(red.as_ptr(), info_level, info) };
        }
    }
    unsafe { call(lp_file_name, info_level, info) }
}

/// FindFirstFileW detour: redirect save-path enumeration/existence checks to the env dir.
pub(super) unsafe extern "system" fn save_redirect_findfirstw_hook(
    lp_file_name: *const u16,
    find_data: *mut c_void,
) -> isize {
    let orig = SAVE_REDIRECT_ORIG_FINDFIRSTW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16, *mut c_void) -> isize = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, *mut c_void) -> isize>(
            orig,
        )
    };
    // RE-ENTRANCY (see `reentry.rs`): same hazard as `GetFileAttributesW` above.
    let depth = SaveDetourDepth::enter();
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 && !depth.is_reentrant() {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("FindFirstFileW", path) {
            return unsafe { call(red.as_ptr(), find_data) };
        }
    }
    unsafe { call(lp_file_name, find_data) }
}

#[cfg(test)]
mod save_container_mode_lock_tests {
    use super::*;
    use er_save_redirect::is_staged_save_container_name;

    /// The mode lock is ASYMMETRIC and must stay that way (user spec 2026-08-02). A symmetric
    /// one-container-per-mode rule is what softlocked the loading screen for 46s on 2026-08-02
    /// (bd `er-effects-rs-h6sh`): a picked `.sl2` staged under a Seamless run that then refused
    /// to load anything but `.co2`.
    ///
    /// Under test there is no live ERSC module, so `save_picker_seamless_mode_after_settle` is
    /// false and this pins the VANILLA half -- the half where a wrong answer is dangerous, because
    /// loading a Seamless `.co2` offline would advance co-op progress in the wrong container.
    #[test]
    fn vanilla_loads_only_sl2_and_never_a_seamless_co2() {
        let names = active_default_save_file_names();
        assert_eq!(
            names,
            &["ER0000.sl2"],
            "vanilla must accept exactly one container and it must be .sl2"
        );
        assert!(
            !names.iter().any(|n| n.eq_ignore_ascii_case("ER0000.co2")),
            "a vanilla run must never load a Seamless .co2"
        );
    }

    /// The write target is a SINGLE container, always drawn from the same mode decision as the
    /// load candidates, and it must be the first (preferred) candidate. That equality is what
    /// guarantees the staged copy, the loaded container and the native write path are one file --
    /// the invariant whose violation caused the softlock.
    #[test]
    fn the_write_target_is_the_preferred_load_candidate() {
        let names = active_default_save_file_names();
        assert_eq!(
            active_default_save_file_name(),
            names[0],
            "the staged/native write name must be the preferred load candidate"
        );
        assert!(
            !names.is_empty(),
            "every mode must name at least one loadable container"
        );
    }

    /// Staging cannot read the mode -- it runs before `ersc.dll` is resident -- so it writes every
    /// container name instead. Whatever this run's mode resolves to, the staged set must already
    /// cover it, or the runtime opens a container staging never wrote (the 2026-08-11 soft lock).
    #[test]
    fn staging_covers_every_container_the_active_mode_can_ask_for() {
        let staged = staged_save_container_names();
        assert!(
            is_staged_save_container_name(active_default_save_file_name(), staged),
            "staging must write the container the native writer targets"
        );
        for name in active_default_save_file_names() {
            assert!(
                is_staged_save_container_name(name, staged),
                "staging must write every container the loader may open: {name}"
            );
        }
        assert!(
            staged.contains(&er_save_redirect::VANILLA_SAVE_CONTAINER_NAME),
            "the vanilla container is always staged, whatever ERSC is configured with"
        );
    }
}

#[cfg(test)]
mod save_redirect_product_reentry_tests {
    use super::*;

    /// The production one-shot must refuse to touch the disk from inside a detour. With no
    /// configured save file it normally reaches the "no configured save file" one-shot log; held
    /// inside a depth token it must bail before even resolving the path, so that latch stays 0.
    #[test]
    fn normalize_one_shot_bails_before_resolving_a_path_inside_a_detour() {
        if configured_save_file().is_some() {
            // A save file is configured in this environment, so the one-shot would take the
            // read-the-file arm and the latch this test reads is not the observable. Skip rather
            // than assert against a different code path.
            return;
        }
        let prior_steam_id = OBSERVED_ACTIVE_STEAM_ID64.load(Ordering::SeqCst);
        let prior_done = SAVE_STEAM_ID_ENV_NORMALIZE_DONE.load(Ordering::SeqCst);
        let prior_logged = SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.load(Ordering::SeqCst);
        // The preconditions the one-shot needs before it does anything at all.
        OBSERVED_ACTIVE_STEAM_ID64.store(76_561_197_960_265_729, Ordering::SeqCst);
        SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(0, Ordering::SeqCst);
        SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.store(0, Ordering::SeqCst);

        {
            let _inside_detour = SaveDetourDepth::enter();
            normalize_env_save_file_to_active_steam_id_once(0, "unit-test-reentrant");
            assert_eq!(
                SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.load(Ordering::SeqCst),
                0,
                "the one-shot ran its body from inside a save-redirect detour"
            );
        }
        normalize_env_save_file_to_active_steam_id_once(0, "unit-test-outside-detour");
        assert_eq!(
            SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.load(Ordering::SeqCst),
            1,
            "the one-shot did not run its body outside a detour"
        );

        OBSERVED_ACTIVE_STEAM_ID64.store(prior_steam_id, Ordering::SeqCst);
        SAVE_STEAM_ID_ENV_NORMALIZE_DONE.store(prior_done, Ordering::SeqCst);
        SAVE_STEAM_ID_NORMALIZE_NO_SOURCE_LOGGED.store(prior_logged, Ordering::SeqCst);
    }
}
