//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use debug::{InputBlocker, InputFlags};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
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
    },
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

/// Convert a Unix absolute path (e.g. `/home/banon/.../save`) to the Wine drive form the in-process
/// `CreateFileW` accepts -- `Z:` maps to `/` under Proton/Wine (confirmed: the game opens our log as
/// `\\?\Z:\home\...`). Backslash separators, no trailing separator. Returns a wide string.
fn unix_path_to_wine_wide(root: &std::path::Path) -> Vec<u16> {
    // to_string_lossy: building a path string, not decoding game memory (the from_utf8_lossy ban
    // targets in-process telemetry; OsStr->String here is fine).
    let win: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let mut out: Vec<u16> = "Z:".encode_utf16().chain(win.encode_utf16()).collect();
    while matches!(out.last(), Some(&c) if c == b'\\' as u16) {
        out.pop();
    }
    out
}

/// Resolve `ER_EFFECTS_SAVE_FILE` -> the staged save ROOT (the ancestor directory that CONTAINS the
/// `EldenRing` folder) in Wine `Z:\...` wide form, or None if the env is unset/blank/not a readable
/// plausibly-sized save / not staged under an `EldenRing` directory component. The redirect rewrites
/// the game's `...\Roaming\EldenRing\<rest>` to `<root>\EldenRing\<rest>`, so the staged save MUST
/// live at `<root>/EldenRing/<steamid>/ER0000.sl2`.
fn save_override_redirect_root_w() -> Option<Vec<u16>> {
    let raw = std::env::var("ER_EFFECTS_SAVE_FILE").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
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
    Some(unix_path_to_wine_wide(&root))
}

/// Outcome of `enforce_save_override_or_abort`. The abort path does not return.
pub(crate) enum SaveOverrideMode {
    /// Pure telemetry/observe-only: no save source required, no redirect installed.
    TelemetryOnly,
    /// A valid env save source was resolved; the redirect hook should be installed.
    Redirect,
}

/// Called EARLY in `DllMain` (before any save IO). Enforces the no-default-fallback rule:
/// unless telemetry-only, a valid `ER_EFFECTS_SAVE_FILE` MUST be present, else the process is
/// aborted immediately. On success it stashes the redirect directory for the CreateFileW hook.
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
                "save-override: FATAL -- ER_EFFECTS_SAVE_FILE is unset/blank/not a readable save (>= {} bytes) staged under an EldenRing dir, and ER_EFFECTS_TELEMETRY_ONLY is not set. Refusing to assume the default user save directory. ABORTING.",
                SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES
            ));
            eprintln!(
                "er-effects: FATAL -- no env-provided save source (ER_EFFECTS_SAVE_FILE) and not telemetry-only; refusing to assume the default user save directory. Aborting."
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
            let d = SAVE_CREATEFILEW_DIAG_LOGGED.load(Ordering::SeqCst);
            if d < SAVE_CREATEFILEW_DIAG_MAX {
                SAVE_CREATEFILEW_DIAG_LOGGED.store(d + 1, Ordering::SeqCst);
                // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
                let p = String::from_utf16_lossy(path);
                append_autoload_debug(format_args!(
                    "save-override: CreateFileW diag call#{calls} save_like={save_like} '{p}'"
                ));
            }
        }
        if let Some(redirected) = save_redirect_path(path) {
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

type ShGetFolderPathWFn = unsafe extern "system" fn(isize, i32, isize, u32, *mut u16) -> i32;

/// SHGetFolderPathW detour: for CSIDL_APPDATA, return our staged ROOT instead of the real %APPDATA%,
/// so the game's save-dir builder produces `<our_root>\EldenRing\<steamid>\...` and reads our gold
/// save's character natively. All other folders pass through unchanged.
unsafe extern "system" fn save_redirect_shgetfolderpathw_hook(
    hwnd: isize,
    csidl: i32,
    token: isize,
    flags: u32,
    path: *mut u16,
) -> i32 {
    const CSIDL_APPDATA: i32 = 0x1a;
    const CSIDL_FOLDER_MASK: i32 = 0xff; // low byte = folder id; high bits = CSIDL_FLAG_*
    const S_OK: i32 = 0;
    const MAX_PATH_W: usize = 259;
    // One-shot: after the first gold load, revert to the real %APPDATA% so writes + subsequent loads
    // use the proper default C: dir (the Z: redirect only serves the first read of the gold).
    if (csidl & CSIDL_FOLDER_MASK) == CSIDL_APPDATA
        && !path.is_null()
        && !SAVE_FIRST_LOAD_DONE.load(Ordering::SeqCst)
    {
        if let Some(root) = SAVE_REDIRECT_DIR_W.get() {
            let n = root.len().min(MAX_PATH_W);
            for i in 0..n {
                unsafe { *path.add(i) = root[i] };
            }
            unsafe { *path.add(n) = 0 };
            let prev = SAVE_REDIRECT_SHGFP_LOGGED.swap(1, Ordering::SeqCst);
            if prev == 0 {
                // UTF-8 Lossy: log-only decode of the staged root for probe confirmation.
                let shown = String::from_utf16_lossy(&root[..n]);
                append_autoload_debug(format_args!(
                    "save-override: SHGetFolderPathW(CSIDL_APPDATA) -> staged root '{shown}' (game now builds all save paths under our tree)"
                ));
            }
            return S_OK;
        }
    }
    let orig = SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW.load(Ordering::SeqCst);
    let call: ShGetFolderPathWFn =
        unsafe { std::mem::transmute::<usize, ShGetFolderPathWFn>(orig) };
    unsafe { call(hwnd, csidl, token, flags, path) }
}

type NtCreateFileFn = unsafe extern "system" fn(
    *mut isize,
    u32,
    *const u8,
    *mut u8,
    *const i64,
    u32,
    u32,
    u32,
    u32,
    *const c_void,
    u32,
) -> i32;

/// NtCreateFile DIAGNOSTIC detour: logs save-LIKE opens (path contains "eldenring" or ends .sl2),
/// including whether the open is RELATIVE to a RootDirectory handle (the invisible-to-Win32 path the
/// game uses for the boot save read). Pure logging -- always calls the original unchanged.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn save_ntcreatefile_diag_hook(
    handle: *mut isize,
    access: u32,
    object_attributes: *const u8,
    iosb: *mut u8,
    alloc: *const i64,
    file_attrs: u32,
    share: u32,
    disposition: u32,
    options: u32,
    ea: *const c_void,
    ea_len: u32,
) -> i32 {
    // OBJECT_ATTRIBUTES (x64): +0x08 RootDirectory (HANDLE), +0x10 ObjectName (PUNICODE_STRING).
    // UNICODE_STRING (x64): +0x00 Length(u16 bytes), +0x08 Buffer(PWSTR).
    // Captured pre-call (path, is_sl2); logged with the NTSTATUS result after the original returns so
    // a FAILING save-commit open is unambiguous (the prior diag logged only the request, never ret).
    let mut save_diag: Option<(String, bool)> = None;
    if !object_attributes.is_null() {
        let objname = unsafe { *(object_attributes.add(0x10) as *const usize) } as *const u8;
        if !objname.is_null() {
            let len_bytes = unsafe { *(objname as *const u16) } as usize;
            let buf = unsafe { *(objname.add(0x08) as *const usize) } as *const u16;
            if !buf.is_null() && len_bytes >= 2 && len_bytes < 0x2000 {
                let nwch = len_bytes / 2;
                let path = unsafe { std::slice::from_raw_parts(buf, nwch) };
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
                // Focus the (capped) budget on ER0000.sl2 opens ONLY -- early boot churns hundreds
                // of "eldenring"-dir opens (graphicsconfig.xml, etc.) that otherwise exhaust the cap
                // before the boot save READ/WRITE we care about. The .sl2 opens ARE the save commit.
                let _ = ELDENRING_SEG;
                let is_sl2 = wide_ends_with_ci_ascii(path, SL2D);
                if is_sl2
                    && SAVE_NTCREATE_DIAG_LOGGED.load(Ordering::SeqCst) < SAVE_NTCREATE_DIAG_MAX
                {
                    // UTF-8 Lossy: log-only decode of an NT path for probe diagnosis.
                    save_diag = Some((String::from_utf16_lossy(path), is_sl2));
                }
            }
        }
    }
    let orig = SAVE_REDIRECT_ORIG_NTCREATEFILE.load(Ordering::SeqCst);
    let call: NtCreateFileFn = unsafe { std::mem::transmute::<usize, NtCreateFileFn>(orig) };
    let ret = unsafe {
        call(
            handle,
            access,
            object_attributes,
            iosb,
            alloc,
            file_attrs,
            share,
            disposition,
            options,
            ea,
            ea_len,
        )
    };
    if let Some((p, is_sl2)) = save_diag {
        let d = SAVE_NTCREATE_DIAG_LOGGED.fetch_add(1, Ordering::SeqCst);
        if d < SAVE_NTCREATE_DIAG_MAX {
            // ret is NTSTATUS (0 == STATUS_SUCCESS). is_write keys off GENERIC_WRITE (0x40000000)
            // or FILE_WRITE_DATA (0x2) so a failing save COMMIT is unambiguous in the log.
            let is_write = access & 0x4000_0000 != 0 || access & 0x2 != 0;
            append_autoload_debug(format_args!(
                "save-override: NtCreateFile diag access=0x{access:x} disp={disposition} opts=0x{options:x} write={is_write} sl2={is_sl2} ret=0x{ret:x} '{p}'"
            ));
        }
    }
    ret
}

type GetDiskFreeSpaceExWFn =
    unsafe extern "system" fn(*const u16, *mut u64, *mut u64, *mut u64) -> i32;

/// GetDiskFreeSpaceExW detour: for the EldenRing save dir, report ample free space (Wine returns
/// bogus 0 on the Z:->/home drive, which fails the save-commit free-space precheck -> corrupted-save
/// loop). Everything else passes through unchanged.
unsafe extern "system" fn save_redirect_getdiskfreew_hook(
    lp_dir: *const u16,
    free_avail: *mut u64,
    total: *mut u64,
    total_free: *mut u64,
) -> i32 {
    // Override EVERY call (the game's save-commit precheck may pass the bare drive root, not an
    // EldenRing path -- diag showed it never matched the eldenring filter). Returning ample free is
    // benign for a probe and guarantees the `free < needed` precheck passes. Log the first few paths.
    const AMPLE_FREE: u64 = 0x10_0000_0000; // 64 GiB
    if !free_avail.is_null() {
        unsafe { *free_avail = AMPLE_FREE };
    }
    if !total.is_null() {
        unsafe { *total = AMPLE_FREE };
    }
    if !total_free.is_null() {
        unsafe { *total_free = AMPLE_FREE };
    }
    let d = SAVE_DISKFREE_LOGGED.fetch_add(1, Ordering::SeqCst);
    if d < 6 {
        let len = unsafe { wide_len(lp_dir) };
        // UTF-8 Lossy: log-only decode of the free-space query path for probe confirmation.
        let p = if len != 0 {
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(lp_dir, len) })
        } else {
            String::new()
        };
        append_autoload_debug(format_args!(
            "save-override: GetDiskFreeSpaceExW #{d} '{p}' -> ample free (unblock save-commit precheck)"
        ));
    }
    1 // TRUE
}

type NtQueryVolumeInfoFn = unsafe extern "system" fn(isize, *mut u8, *mut u8, u32, u32) -> i32;

/// NtQueryVolumeInformationFile detour: override the AVAILABLE free-space units for the size info
/// classes so the save-commit precheck passes (Wine reports bogus 0 free on the Z: staged drive).
unsafe extern "system" fn save_redirect_ntqueryvolinfo_hook(
    handle: isize,
    iosb: *mut u8,
    fs_info: *mut u8,
    length: u32,
    fs_class: u32,
) -> i32 {
    const FILE_FS_SIZE_INFORMATION: u32 = 3;
    const FILE_FS_FULL_SIZE_INFORMATION: u32 = 7;
    const AMPLE_UNITS: i64 = 0x1000_0000; // ~268M allocation units -> ample free regardless of unit size
    let orig = SAVE_REDIRECT_ORIG_NTQUERYVOLINFO.load(Ordering::SeqCst);
    let call: NtQueryVolumeInfoFn =
        unsafe { std::mem::transmute::<usize, NtQueryVolumeInfoFn>(orig) };
    let ret = unsafe { call(handle, iosb, fs_info, length, fs_class) };
    // DIAGNOSTIC: log only the FREE-SPACE classes (3/7), capped. Logging every class exhausts the cap
    // on early-boot class=1 spam before the save-time free-space precheck fires; the precheck is the
    // only thing that matters for the corrupted-save loop. pre_avail_units = the bogus Wine value.
    if fs_class == FILE_FS_SIZE_INFORMATION || fs_class == FILE_FS_FULL_SIZE_INFORMATION {
        let d = SAVE_VOLINFO_LOGGED.load(Ordering::SeqCst);
        if d < 40 {
            SAVE_VOLINFO_LOGGED.store(d + 1, Ordering::SeqCst);
            let avail = if ret == 0 && !fs_info.is_null() && length >= 16 {
                unsafe { *(fs_info.add(8) as *const i64) }
            } else {
                -1
            };
            append_autoload_debug(format_args!(
                "save-override: NtQueryVolumeInformationFile diag class={fs_class} len={length} ret=0x{ret:x} pre_avail_units={avail}"
            ));
        }
    }
    if ret == 0 && !fs_info.is_null() {
        if fs_class == FILE_FS_SIZE_INFORMATION && length >= 16 {
            // [+0] TotalAllocationUnits (i64), [+8] AvailableAllocationUnits (i64).
            unsafe {
                *(fs_info.add(0) as *mut i64) = AMPLE_UNITS;
                *(fs_info.add(8) as *mut i64) = AMPLE_UNITS;
            }
        } else if fs_class == FILE_FS_FULL_SIZE_INFORMATION && length >= 24 {
            // [+0] Total, [+8] CallerAvailable, [+16] ActualAvailable (all i64).
            unsafe {
                *(fs_info.add(0) as *mut i64) = AMPLE_UNITS;
                *(fs_info.add(8) as *mut i64) = AMPLE_UNITS;
                *(fs_info.add(16) as *mut i64) = AMPLE_UNITS;
            }
        } else {
            return ret;
        }
        let d = SAVE_VOLINFO_LOGGED.fetch_add(1, Ordering::SeqCst);
        if d < 4 {
            append_autoload_debug(format_args!(
                "save-override: NtQueryVolumeInformationFile class={fs_class} -> ample free units (unblock save-commit precheck) #{d}"
            ));
        }
    }
    ret
}

/// True when running under Wine/Proton (ntdll exports `wine_get_version`, which native Windows does
/// not). The free-space-precheck workaround is a Wine-specific bug fix (Wine reports bogus 0 free for
/// the Z:->/home drive mapping); on native Windows it must NOT run (it would mask a real disk-full).
pub(crate) fn running_under_wine() -> bool {
    unsafe { module_proc(b"ntdll.dll\0", b"wine_get_version\0") != HOOK_ORIGINAL_UNSET }
}

/// Resolve an export address from an already-loaded module (NUL-terminated ASCII names). 0 if the
/// module isn't loaded or the export is absent.
unsafe fn module_proc(module_name: &[u8], proc_name: &[u8]) -> usize {
    let module = match unsafe { GetModuleHandleA(PCSTR::from_raw(module_name.as_ptr())) } {
        Ok(m) => m,
        Err(_) => return HOOK_ORIGINAL_UNSET,
    };
    match unsafe { GetProcAddress(module, PCSTR::from_raw(proc_name.as_ptr())) } {
        Some(p) => p as usize,
        None => HOOK_ORIGINAL_UNSET,
    }
}

/// Resolve a kernel32 export address by name (NUL-terminated ASCII). 0 if unavailable.
unsafe fn kernel32_proc(name: &[u8]) -> usize {
    unsafe { module_proc(b"kernel32.dll\0", name) }
}

/// Install the save-redirect hooks (CreateFileW + CopyFileW) ONCE. Idempotent. Must run while the
/// redirect dir is already stashed (after `enforce_save_override_or_abort` -> Redirect). Mirrors the
/// thread-spawn install pattern of the other early DllMain subsystems.
/// Queue one kernel32 export hook (resolve by name, store trampoline, queue-enable). Best-effort:
/// logs and skips on any failure. Used for the save-redirect existence-check APIs.
unsafe fn queue_save_redirect_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    proc_name: &[u8],
    detour: *mut c_void,
    orig: &AtomicUsize,
) {
    let addr = unsafe { kernel32_proc(proc_name) };
    if addr == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "save-override: could not resolve kernel32!{name}"
        ));
        return;
    }
    match unsafe { MhHook::new(addr as *mut c_void, detour) } {
        Ok(hook) => {
            orig.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-override: {name} queue_enable failed: {status:?}"
                ));
            } else {
                hooks.push(hook);
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-override: MhHook::new {name} failed at 0x{addr:x}: {status:?}"
        )),
    }
}

pub(crate) fn install_save_redirect_hooks() {
    SAVE_REDIRECT_INSTALL_ONCE.call_once(|| {
        if SAVE_REDIRECT_DIR_W.get().is_none() && !save_trace_enabled() {
            append_autoload_debug(format_args!(
                "save-override: install skipped -- redirect dir not set (enforce did not run / telemetry-only)"
            ));
            return;
        }
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => {
                append_autoload_debug(format_args!(
                    "save-override: MH_Initialize failed: {status:?}"
                ));
                return;
            }
        }
        append_autoload_debug(format_args!(
            "save-override: install begin -- running_under_wine={} (Wine-only free-space overrides {})",
            running_under_wine(),
            if running_under_wine() { "ARMED" } else { "SKIPPED" }
        ));
        let mut hooks = Vec::new();
        let create_addr = unsafe { kernel32_proc(b"CreateFileW\0") };
        if create_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    create_addr as *mut c_void,
                    save_redirect_createfilew_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_CREATEFILEW
                        .store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: CreateFileW queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new CreateFileW failed at 0x{create_addr:x}: {status:?}"
                )),
            }
        } else {
            append_autoload_debug(format_args!(
                "save-override: could not resolve kernel32!CreateFileW"
            ));
        }
        let copy_addr = unsafe { kernel32_proc(b"CopyFileW\0") };
        if copy_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    copy_addr as *mut c_void,
                    save_redirect_copyfilew_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_COPYFILEW.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: CopyFileW queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new CopyFileW failed at 0x{copy_addr:x}: {status:?}"
                )),
            }
        }
        // Existence-check redirects: the game stats/enumerates ER0000.sl2 before opening it; without
        // these the wiped default dir reads as "no save" and CreateFileW is never reached.
        unsafe {
            queue_save_redirect_hook(
                &mut hooks,
                "GetFileAttributesW",
                b"GetFileAttributesW\0",
                save_redirect_getattrw_hook as *mut c_void,
                &SAVE_REDIRECT_ORIG_GETATTRW,
            );
            queue_save_redirect_hook(
                &mut hooks,
                "GetFileAttributesExW",
                b"GetFileAttributesExW\0",
                save_redirect_getattrexw_hook as *mut c_void,
                &SAVE_REDIRECT_ORIG_GETATTREXW,
            );
            queue_save_redirect_hook(
                &mut hooks,
                "FindFirstFileW",
                b"FindFirstFileW\0",
                save_redirect_findfirstw_hook as *mut c_void,
                &SAVE_REDIRECT_ORIG_FINDFIRSTW,
            );
            // THE corruption fix (WINE ONLY): ample free space for the save dir (Wine Z: drive reports
            // bogus 0). Native Windows reports correctly, so this must not run there.
            if running_under_wine() {
                queue_save_redirect_hook(
                    &mut hooks,
                    "GetDiskFreeSpaceExW",
                    b"GetDiskFreeSpaceExW\0",
                    save_redirect_getdiskfreew_hook as *mut c_void,
                    &SAVE_REDIRECT_ORIG_GETDISKFREEW,
                );
            }
        }
        // PRIMARY: redirect the %APPDATA% root via SHGetFolderPathW (shell32) so the game builds and
        // opens the full save path under our staged tree natively -- this is what actually makes the
        // character load (the per-file kernel32 hooks above are a fallback for the real default dir).
        let shgfp_addr = unsafe { module_proc(b"shell32.dll\0", b"SHGetFolderPathW\0") };
        if shgfp_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    shgfp_addr as *mut c_void,
                    save_redirect_shgetfolderpathw_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW
                        .store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: SHGetFolderPathW queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new SHGetFolderPathW failed at 0x{shgfp_addr:x}: {status:?}"
                )),
            }
        } else {
            append_autoload_debug(format_args!(
                "save-override: could not resolve shell32!SHGetFolderPathW (shell32 not loaded yet?)"
            ));
        }
        // THE corruption fix at the lowest layer (WINE ONLY): ntdll!NtQueryVolumeInformationFile
        // free-space override (the game's free-space precheck never reaches our kernel32 hook). Native
        // Windows reports free space correctly, so this Wine-bug workaround must not run there.
        let ntqvi_addr = if running_under_wine() {
            unsafe { module_proc(b"ntdll.dll\0", b"NtQueryVolumeInformationFile\0") }
        } else {
            HOOK_ORIGINAL_UNSET
        };
        if ntqvi_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    ntqvi_addr as *mut c_void,
                    save_redirect_ntqueryvolinfo_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_NTQUERYVOLINFO
                        .store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: NtQueryVolumeInformationFile queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new NtQueryVolumeInformationFile failed at 0x{ntqvi_addr:x}: {status:?}"
                )),
            }
        } else {
            append_autoload_debug(format_args!(
                "save-override: could not resolve ntdll!NtQueryVolumeInformationFile"
            ));
        }
        // DIAGNOSTIC: ntdll!NtCreateFile -- see the boot save read that is invisible to Win32.
        let ntcf_addr = unsafe { module_proc(b"ntdll.dll\0", b"NtCreateFile\0") };
        if ntcf_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    ntcf_addr as *mut c_void,
                    save_ntcreatefile_diag_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_NTCREATEFILE.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: NtCreateFile queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new NtCreateFile failed at 0x{ntcf_addr:x}: {status:?}"
                )),
            }
        }
        match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => append_autoload_debug(format_args!(
                "save-override: INSTALLED SHGetFolderPathW(0x{shgfp_addr:x})+CreateFileW(0x{create_addr:x})+CopyFileW(0x{copy_addr:x})+GetFileAttributesW/ExW+FindFirstFileW save-path redirect -- default user save dir is now never read"
            )),
            status => append_autoload_debug(format_args!(
                "save-override: MH_ApplyQueued failed: {status:?}"
            )),
        }
        std::mem::forget(hooks);
    });
}
