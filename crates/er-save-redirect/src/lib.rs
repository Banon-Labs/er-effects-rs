//! Shared save-source/redirect planning core.
//!
//! This is S6b.1: host-runnable state and source planning only. It deliberately does not install
//! Win32/NT save hooks and does not own boot/title-flow gates. Those are process-wide runtime
//! ownership questions for later slices.

mod reentry;
pub use reentry::{SaveDetourDepth, SaveNtCreateDetourGuard, save_detour_disk_io_allowed};

use std::{
    ffi::c_void,
    path::{Path, PathBuf},
    sync::{
        Once,
        atomic::{AtomicUsize, Ordering},
    },
};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

/// Exact byte length of Elden Ring PC `ER0000.sl2` / Seamless `.co2` save containers.
///
/// These files use a fixed BND4 layout: ten `USER_DATA00N` character slots, `USER_DATA010`, and
/// `USER_DATA011`. A different length is not a valid Elden Ring save container for this loader.
pub const EXPECTED_SAVE_FILE_BYTES: u64 = 0x1ba03d0;

/// Missing-save gate state shared by picker, redirect activation, and boot hold owners inside one
/// DLL image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSaveState {
    Idle,
    Pending,
    Ready,
}

impl MissingSaveState {
    const fn as_usize(self) -> usize {
        match self {
            Self::Idle => 0,
            Self::Pending => 1,
            Self::Ready => 2,
        }
    }

    const fn from_usize(value: usize) -> Self {
        match value {
            1 => Self::Pending,
            2 => Self::Ready,
            _ => Self::Idle,
        }
    }
}

/// Atomic holder for the missing-save state machine.
pub struct MissingSaveGate {
    state: AtomicUsize,
}

impl MissingSaveGate {
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(MissingSaveState::Idle.as_usize()),
        }
    }

    pub fn set(&self, state: MissingSaveState) {
        self.state.store(state.as_usize(), Ordering::SeqCst);
    }

    pub fn state(&self) -> MissingSaveState {
        MissingSaveState::from_usize(self.state.load(Ordering::SeqCst))
    }

    pub fn is_pending(&self) -> bool {
        self.state() == MissingSaveState::Pending
    }
}

impl Default for MissingSaveGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-local save hook install gate state.
///
/// This does not install MinHook detours by itself. It owns the shared once/installed-state shape so
/// product and later standalone hook-owner code use the same idempotency contract.
pub struct SaveHookInstallState {
    core_once: Once,
    redirect_once: Once,
    core_createfilew_installed: AtomicUsize,
}

impl SaveHookInstallState {
    pub const fn new() -> Self {
        Self {
            core_once: Once::new(),
            redirect_once: Once::new(),
            core_createfilew_installed: AtomicUsize::new(0),
        }
    }

    pub fn install_core_once(&self, install: impl FnOnce()) {
        self.core_once.call_once(install);
    }

    pub fn install_redirect_once(&self, install: impl FnOnce()) {
        self.redirect_once.call_once(install);
    }

    pub fn mark_core_createfilew_installed(&self) {
        self.core_createfilew_installed.store(1, Ordering::SeqCst);
    }

    pub fn core_createfilew_installed(&self) -> bool {
        self.core_createfilew_installed.load(Ordering::SeqCst) != 0
    }
}

impl Default for SaveHookInstallState {
    fn default() -> Self {
        Self::new()
    }
}

/// Original/trampoline slot value before a hook is installed.
pub const SAVE_HOOK_ORIGINAL_UNSET: usize = 0;

/// Original CreateFileW / CopyFileW MinHook trampolines. 0 = not hooked.
pub static SAVE_REDIRECT_ORIG_CREATEFILEW: AtomicUsize = AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
pub static SAVE_REDIRECT_ORIG_COPYFILEW: AtomicUsize = AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
/// Save-existence-check redirect trampolines.
pub static SAVE_REDIRECT_ORIG_GETATTRW: AtomicUsize = AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
pub static SAVE_REDIRECT_ORIG_GETATTREXW: AtomicUsize = AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
pub static SAVE_REDIRECT_ORIG_FINDFIRSTW: AtomicUsize = AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
/// SHGetFolderPathW redirect trampoline.
pub static SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW: AtomicUsize =
    AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
/// Ntdll/save-destination diagnostics and free-space override trampolines.
pub static SAVE_REDIRECT_ORIG_NTCREATEFILE: AtomicUsize =
    AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
pub static SAVE_REDIRECT_ORIG_GETDISKFREEW: AtomicUsize =
    AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);
pub static SAVE_REDIRECT_ORIG_NTQUERYVOLINFO: AtomicUsize =
    AtomicUsize::new(SAVE_HOOK_ORIGINAL_UNSET);

/// Queue one already-resolved save hook target and store its trampoline.
///
/// Address resolution and logging remain caller-owned so product and future standalone owners can use
/// different module/export lookup and telemetry sinks while sharing the MinHook queue/slot contract.
///
/// # Safety
/// `target_addr` and `detour` must be valid for MinHook in the current process, and `detour` must
/// match the target function ABI.
pub unsafe fn queue_resolved_save_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    target_addr: usize,
    detour: *mut c_void,
    orig: &AtomicUsize,
    mut log: impl FnMut(String),
) {
    if target_addr == SAVE_HOOK_ORIGINAL_UNSET {
        log(format!("save-override: could not resolve {name}"));
        return;
    }
    match unsafe { MhHook::new(target_addr as *mut c_void, detour) } {
        Ok(hook) => {
            orig.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                log(format!(
                    "save-override: {name} queue_enable failed: {status:?}"
                ));
            } else {
                hooks.push(hook);
            }
        }
        Err(status) => log(format!(
            "save-override: MhHook::new {name} failed at 0x{target_addr:x}: {status:?}"
        )),
    }
}

/// Install the always-on core CreateFileW save hook once.
///
/// Product still supplies export resolution, the detour function pointer, and the log sink. The
/// shared redirect core owns the idempotency, MinHook initialization, trampoline storage, queue
/// enable, apply, and live-state mark.
///
/// # Safety
/// `createfilew_detour` must match the Win32 `CreateFileW` ABI and must remain valid for the process
/// lifetime.
pub unsafe fn install_core_createfilew_hook(
    state: &SaveHookInstallState,
    createfilew_detour: *mut c_void,
    resolve_kernel32: impl FnOnce(&[u8]) -> usize,
    mut log: impl FnMut(String),
) {
    state.install_core_once(|| {
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => {
                log(format!("save-override: core MH_Initialize failed: {status:?}"));
                return;
            }
        }
        let create_addr = resolve_kernel32(b"CreateFileW\0");
        if create_addr == SAVE_HOOK_ORIGINAL_UNSET {
            log("save-override: core could not resolve kernel32!CreateFileW -- save-destination commits cannot redirect their write-open".to_owned());
            return;
        }
        let hook = match unsafe {
            MhHook::new(create_addr as *mut c_void, createfilew_detour)
        } {
            Ok(hook) => hook,
            Err(status) => {
                log(format!(
                    "save-override: core MhHook::new CreateFileW failed at 0x{create_addr:x}: {status:?}"
                ));
                return;
            }
        };
        SAVE_REDIRECT_ORIG_CREATEFILEW.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            log(format!("save-override: core CreateFileW queue_enable failed: {status:?}"));
            return;
        }
        match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                state.mark_core_createfilew_installed();
                std::mem::forget(hook);
                log(format!(
                    "save-override: core INSTALLED CreateFileW(0x{create_addr:x}) -- pass-through until a redirect dir or a save destination is armed"
                ));
            }
            status => log(format!(
                "save-override: core CreateFileW MH_ApplyQueued failed: {status:?}"
            )),
        }
    });
}

/// Detour entry points for the redirect-mode save hook batch.
pub struct SaveRedirectHookDetours {
    pub copyfilew: *mut c_void,
    pub get_file_attributes_w: *mut c_void,
    pub get_file_attributes_ex_w: *mut c_void,
    pub find_first_file_w: *mut c_void,
    pub get_disk_free_space_ex_w: *mut c_void,
    pub sh_get_folder_path_w: *mut c_void,
    pub nt_query_volume_information_file: *mut c_void,
    pub nt_create_file: *mut c_void,
}

/// Install the redirect-mode save hook batch once.
///
/// Product still decides when redirect mode is armed, supplies module/export resolution, supplies
/// detour entry points, and logs to its telemetry sink. The shared core owns the idempotent MinHook
/// initialization, queue/store/apply sequence for the redirect batch.
///
/// # Safety
/// Every detour pointer must match the ABI of the target function resolved for it and remain valid
/// for the process lifetime. The `install_core_createfilew` callback must install the matching core
/// `CreateFileW` hook before this batch needs to redirect save opens.
/// Install the redirect-mode save hook batch only after a redirect root or trace mode is ready.
///
/// This keeps the native no-save picker boot path unhooked until the product has activated a picked
/// source, while still allowing explicit trace mode to install the observation hooks.
///
/// # Safety
/// Same requirements as [`install_redirect_save_hooks`].
pub unsafe fn install_redirect_save_hooks_when_ready(
    state: &SaveHookInstallState,
    redirect_root_ready: bool,
    trace_enabled: bool,
    detours: SaveRedirectHookDetours,
    running_under_wine: bool,
    resolve_kernel32: impl FnMut(&[u8]) -> usize,
    resolve_shell32: impl FnMut(&[u8]) -> usize,
    resolve_ntdll: impl FnMut(&[u8]) -> usize,
    install_core_createfilew: impl FnOnce(),
    mut log: impl FnMut(String),
) {
    if !redirect_root_ready && !trace_enabled {
        log("save-override: install deferred -- redirect dir not set yet (waiting for missing-save picker/configured source)".to_owned());
        return;
    }
    unsafe {
        install_redirect_save_hooks(
            state,
            detours,
            running_under_wine,
            resolve_kernel32,
            resolve_shell32,
            resolve_ntdll,
            install_core_createfilew,
            log,
        );
    }
}

pub unsafe fn install_redirect_save_hooks(
    state: &SaveHookInstallState,
    detours: SaveRedirectHookDetours,
    running_under_wine: bool,
    mut resolve_kernel32: impl FnMut(&[u8]) -> usize,
    mut resolve_shell32: impl FnMut(&[u8]) -> usize,
    mut resolve_ntdll: impl FnMut(&[u8]) -> usize,
    install_core_createfilew: impl FnOnce(),
    mut log: impl FnMut(String),
) {
    state.install_redirect_once(|| {
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => {
                log(format!("save-override: MH_Initialize failed: {status:?}"));
                return;
            }
        }
        log(format!(
            "save-override: install begin -- running_under_wine={} (Wine-only free-space overrides {})",
            running_under_wine,
            if running_under_wine { "ARMED" } else { "SKIPPED" }
        ));
        let mut hooks = Vec::new();
        install_core_createfilew();
        let create_addr = if state.core_createfilew_installed() {
            resolve_kernel32(b"CreateFileW\0")
        } else {
            log("save-override: CreateFileW detour is NOT live (core install failed) -- save-path redirection cannot work".to_owned());
            SAVE_HOOK_ORIGINAL_UNSET
        };

        let copy_addr = resolve_kernel32(b"CopyFileW\0");
        if copy_addr != SAVE_HOOK_ORIGINAL_UNSET {
            unsafe {
                queue_resolved_save_hook(
                    &mut hooks,
                    "CopyFileW",
                    copy_addr,
                    detours.copyfilew,
                    &SAVE_REDIRECT_ORIG_COPYFILEW,
                    &mut log,
                );
            }
        }

        unsafe {
            queue_kernel32_save_redirect_hook(
                &mut hooks,
                "GetFileAttributesW",
                b"GetFileAttributesW\0",
                detours.get_file_attributes_w,
                &SAVE_REDIRECT_ORIG_GETATTRW,
                &mut resolve_kernel32,
                &mut log,
            );
            queue_kernel32_save_redirect_hook(
                &mut hooks,
                "GetFileAttributesExW",
                b"GetFileAttributesExW\0",
                detours.get_file_attributes_ex_w,
                &SAVE_REDIRECT_ORIG_GETATTREXW,
                &mut resolve_kernel32,
                &mut log,
            );
            queue_kernel32_save_redirect_hook(
                &mut hooks,
                "FindFirstFileW",
                b"FindFirstFileW\0",
                detours.find_first_file_w,
                &SAVE_REDIRECT_ORIG_FINDFIRSTW,
                &mut resolve_kernel32,
                &mut log,
            );
            if running_under_wine {
                queue_kernel32_save_redirect_hook(
                    &mut hooks,
                    "GetDiskFreeSpaceExW",
                    b"GetDiskFreeSpaceExW\0",
                    detours.get_disk_free_space_ex_w,
                    &SAVE_REDIRECT_ORIG_GETDISKFREEW,
                    &mut resolve_kernel32,
                    &mut log,
                );
            }
        }

        let shgfp_addr = resolve_shell32(b"SHGetFolderPathW\0");
        if shgfp_addr != SAVE_HOOK_ORIGINAL_UNSET {
            unsafe {
                queue_resolved_save_hook(
                    &mut hooks,
                    "SHGetFolderPathW",
                    shgfp_addr,
                    detours.sh_get_folder_path_w,
                    &SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW,
                    &mut log,
                );
            }
        } else {
            log("save-override: could not resolve shell32!SHGetFolderPathW (shell32 not loaded yet?)".to_owned());
        }

        let ntqvi_addr = if running_under_wine {
            resolve_ntdll(b"NtQueryVolumeInformationFile\0")
        } else {
            SAVE_HOOK_ORIGINAL_UNSET
        };
        if ntqvi_addr != SAVE_HOOK_ORIGINAL_UNSET {
            unsafe {
                queue_resolved_save_hook(
                    &mut hooks,
                    "NtQueryVolumeInformationFile",
                    ntqvi_addr,
                    detours.nt_query_volume_information_file,
                    &SAVE_REDIRECT_ORIG_NTQUERYVOLINFO,
                    &mut log,
                );
            }
        } else {
            log("save-override: could not resolve ntdll!NtQueryVolumeInformationFile".to_owned());
        }

        let ntcf_addr = resolve_ntdll(b"NtCreateFile\0");
        if ntcf_addr != SAVE_HOOK_ORIGINAL_UNSET {
            unsafe {
                queue_resolved_save_hook(
                    &mut hooks,
                    "NtCreateFile",
                    ntcf_addr,
                    detours.nt_create_file,
                    &SAVE_REDIRECT_ORIG_NTCREATEFILE,
                    &mut log,
                );
            }
        }

        match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => log(format!(
                "save-override: INSTALLED SHGetFolderPathW(0x{shgfp_addr:x})+CreateFileW(0x{create_addr:x})+CopyFileW(0x{copy_addr:x})+GetFileAttributesW/ExW+FindFirstFileW save-path redirect -- default user save dir is now never read"
            )),
            status => log(format!("save-override: MH_ApplyQueued failed: {status:?}")),
        }
        std::mem::forget(hooks);
    });
}

unsafe fn queue_kernel32_save_redirect_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    proc_name: &[u8],
    detour: *mut c_void,
    orig: &AtomicUsize,
    resolve_kernel32: &mut impl FnMut(&[u8]) -> usize,
    log: &mut impl FnMut(String),
) {
    let addr = resolve_kernel32(proc_name);
    if addr == SAVE_HOOK_ORIGINAL_UNSET {
        log(format!("save-override: could not resolve kernel32!{name}"));
        return;
    }
    unsafe {
        queue_resolved_save_hook(hooks, name, addr, detour, orig, log);
    }
}

/// Ample byte count reported by the Wine free-space workaround (`64 GiB`).
pub const SAVE_REDIRECT_AMPLE_FREE_BYTES: u64 = 0x10_0000_0000;
/// `FILE_FS_SIZE_INFORMATION` class id for `NtQueryVolumeInformationFile`.
pub const FILE_FS_SIZE_INFORMATION_CLASS: u32 = 3;
/// `FILE_FS_FULL_SIZE_INFORMATION` class id for `NtQueryVolumeInformationFile`.
pub const FILE_FS_FULL_SIZE_INFORMATION_CLASS: u32 = 7;
/// Ample allocation-unit count reported by the Wine `NtQueryVolumeInformationFile` workaround.
pub const SAVE_REDIRECT_AMPLE_FREE_UNITS: i64 = 0x1000_0000;

/// Fill `GetDiskFreeSpaceExW` output pointers with ample free space.
///
/// # Safety
/// Non-null pointers must be valid writable `u64` outputs for the duration of the call.
pub unsafe fn fill_get_disk_free_space_ex_outputs(
    free_avail: *mut u64,
    total: *mut u64,
    total_free: *mut u64,
) {
    if !free_avail.is_null() {
        unsafe { *free_avail = SAVE_REDIRECT_AMPLE_FREE_BYTES };
    }
    if !total.is_null() {
        unsafe { *total = SAVE_REDIRECT_AMPLE_FREE_BYTES };
    }
    if !total_free.is_null() {
        unsafe { *total_free = SAVE_REDIRECT_AMPLE_FREE_BYTES };
    }
}

pub fn is_ntquery_volume_free_space_class(fs_class: u32) -> bool {
    fs_class == FILE_FS_SIZE_INFORMATION_CLASS || fs_class == FILE_FS_FULL_SIZE_INFORMATION_CLASS
}

/// Read the pre-patch available-unit field for the free-space info classes.
///
/// # Safety
/// `fs_info` must point at the buffer returned by `NtQueryVolumeInformationFile` when non-null.
pub unsafe fn ntquery_volume_available_units(
    ret: i32,
    fs_info: *const u8,
    length: u32,
    fs_class: u32,
) -> Option<i64> {
    if ret == 0
        && !fs_info.is_null()
        && is_ntquery_volume_free_space_class(fs_class)
        && length >= 16
    {
        Some(unsafe { *(fs_info.add(8) as *const i64) })
    } else {
        None
    }
}

/// Patch free-space fields in `NtQueryVolumeInformationFile` output to ample units.
///
/// Returns true when it recognized and patched a free-space info class.
///
/// # Safety
/// `fs_info` must point at the mutable buffer returned by `NtQueryVolumeInformationFile` when
/// non-null, and `length` must describe the writable byte length.
pub unsafe fn patch_ntquery_volume_free_space(
    ret: i32,
    fs_info: *mut u8,
    length: u32,
    fs_class: u32,
) -> bool {
    if ret != 0 || fs_info.is_null() {
        return false;
    }
    if fs_class == FILE_FS_SIZE_INFORMATION_CLASS && length >= 16 {
        // [+0] TotalAllocationUnits (i64), [+8] AvailableAllocationUnits (i64).
        unsafe {
            *(fs_info.add(0) as *mut i64) = SAVE_REDIRECT_AMPLE_FREE_UNITS;
            *(fs_info.add(8) as *mut i64) = SAVE_REDIRECT_AMPLE_FREE_UNITS;
        }
        true
    } else if fs_class == FILE_FS_FULL_SIZE_INFORMATION_CLASS && length >= 24 {
        // [+0] Total, [+8] CallerAvailable, [+16] ActualAvailable (all i64).
        unsafe {
            *(fs_info.add(0) as *mut i64) = SAVE_REDIRECT_AMPLE_FREE_UNITS;
            *(fs_info.add(8) as *mut i64) = SAVE_REDIRECT_AMPLE_FREE_UNITS;
            *(fs_info.add(16) as *mut i64) = SAVE_REDIRECT_AMPLE_FREE_UNITS;
        }
        true
    } else {
        false
    }
}

/// Low-byte folder id for `%APPDATA%` in `SHGetFolderPathW` requests.
pub const SHGFP_CSIDL_APPDATA: i32 = 0x1a;
/// Mask for extracting the folder id from a `SHGetFolderPathW` CSIDL value.
pub const SHGFP_CSIDL_FOLDER_MASK: i32 = 0xff;
/// Product-side `SHGetFolderPathW` buffer capacity used by the save redirect hook.
pub const SHGFP_MAX_PATH_W: usize = 259;

pub fn shgetfolderpath_is_appdata_request(csidl: i32) -> bool {
    (csidl & SHGFP_CSIDL_FOLDER_MASK) == SHGFP_CSIDL_APPDATA
}

pub fn shgetfolderpath_staged_appdata_len(
    csidl: i32,
    first_load_done: bool,
    staged_root_len: Option<usize>,
    max_path_w: usize,
) -> Option<usize> {
    if shgetfolderpath_is_appdata_request(csidl) && !first_load_done {
        staged_root_len.map(|len| len.min(max_path_w))
    } else {
        None
    }
}

/// Copy the staged root into a `SHGetFolderPathW` output buffer and NUL-terminate it.
///
/// # Safety
/// `path` must point at a writable buffer with room for `n + 1` UTF-16 code units, where `n` is the
/// returned copy length.
pub unsafe fn write_shgetfolderpath_staged_root(path: *mut u16, root: &[u16], n: usize) -> usize {
    let n = n.min(root.len());
    for (i, ch) in root.iter().copied().take(n).enumerate() {
        unsafe { *path.add(i) = ch };
    }
    unsafe { *path.add(n) = 0 };
    n
}

/// `GENERIC_WRITE` access bit used by NtCreateFile diagnostics.
pub const NT_CREATEFILE_GENERIC_WRITE: u32 = 0x4000_0000;
/// `FILE_WRITE_DATA` access bit used by NtCreateFile diagnostics.
pub const NT_CREATEFILE_FILE_WRITE_DATA: u32 = 0x2;

/// Shared classification for the NtCreateFile diagnostic detour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NtCreateFileSavePathDiag {
    pub is_save_file_or_backup: bool,
    pub is_sl2: bool,
    pub is_write: bool,
}

impl NtCreateFileSavePathDiag {
    pub fn should_wait_for_missing_save_dialog(self) -> bool {
        self.is_save_file_or_backup
    }

    pub fn should_observe_steam_id(self) -> bool {
        self.is_sl2
    }

    pub fn should_normalize_on_read(self) -> bool {
        self.is_sl2 && !self.is_write
    }

    pub fn should_capture_diag_log(self, logged: usize, max: usize) -> bool {
        self.is_sl2 && logged < max
    }
}

pub fn nt_createfile_access_is_write(access: u32) -> bool {
    access & NT_CREATEFILE_GENERIC_WRITE != 0 || access & NT_CREATEFILE_FILE_WRITE_DATA != 0
}

pub fn classify_nt_create_file_save_path(path: &[u16], access: u32) -> NtCreateFileSavePathDiag {
    const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
    NtCreateFileSavePathDiag {
        is_save_file_or_backup: is_save_file_or_backup_path(path),
        is_sl2: wide_ends_with_ci_ascii(path, SL2D),
        is_write: nt_createfile_access_is_write(access),
    }
}

pub fn nt_createfile_diag_hit_should_log(hits: usize) -> bool {
    hits <= 8 || hits.is_power_of_two()
}

/// Shared classification for CreateFileW save redirect diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateFileSavePathDiag {
    pub save_like: bool,
    pub is_save_file: bool,
    pub should_wait_for_missing_save_dialog: bool,
}

impl CreateFileSavePathDiag {
    pub fn should_capture_diag_log(self, calls: usize) -> bool {
        calls == 0 || self.save_like
    }
}

pub fn classify_create_file_save_path(path: &[u16]) -> CreateFileSavePathDiag {
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
    let is_save_file = wide_ends_with_ci_ascii(path, SL2D) || wide_ends_with_ci_ascii(path, CO2D);
    let is_backup = wide_ends_with_ci_ascii(path, BAKD);
    CreateFileSavePathDiag {
        save_like: wide_contains_ci_ascii(path, ELDENRING_SEG) || is_save_file || is_backup,
        is_save_file,
        should_wait_for_missing_save_dialog: is_save_file || is_backup,
    }
}

pub fn createfile_diag_hit_should_log(hits: usize) -> bool {
    hits <= 8 || hits.is_power_of_two()
}

/// Why a candidate save source was rejected before redirect planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveSourceRejection {
    MissingOrNotFile,
    WrongSize { len: u64, expected: u64 },
    NotBnd4,
    Unreadable,
}

/// UTF-16 Wine/Windows save-root path without a trailing separator or NUL terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineRootWide(Vec<u16>);

impl WineRootWide {
    pub fn as_slice(&self) -> &[u16] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<u16> {
        self.0
    }
}

/// Host-runnable source plan. Runtime hook installation is intentionally outside this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveSourcePlan {
    /// Source already lives under `<root>/EldenRing/<steamid>/ER0000.*` and the product is allowed
    /// to normalize/write it in place, so redirect the native save root to that staged root.
    StagedRoot {
        file: PathBuf,
        steam_id: u64,
        root_wide: WineRootWide,
    },
    /// Source is an arbitrary save file. Stage it under a private native save root; the source file
    /// remains read-only from the game's point of view.
    DirectFile {
        file: PathBuf,
        stage_root: PathBuf,
        root_wide: WineRootWide,
    },
}

impl SaveSourcePlan {
    pub fn file(&self) -> &Path {
        match self {
            Self::StagedRoot { file, .. } | Self::DirectFile { file, .. } => file,
        }
    }

    pub fn root_wide(&self) -> &WineRootWide {
        match self {
            Self::StagedRoot { root_wide, .. } | Self::DirectFile { root_wide, .. } => root_wide,
        }
    }
}

/// Validate a candidate picked/configured save. This is stronger than size-only: it also proves the
/// file is a structurally readable BND4 container.
pub fn validate_save_file_path(path: PathBuf) -> Result<PathBuf, SaveSourceRejection> {
    let meta = std::fs::metadata(&path).map_err(|_| SaveSourceRejection::MissingOrNotFile)?;
    if !meta.is_file() {
        return Err(SaveSourceRejection::MissingOrNotFile);
    }
    if meta.len() != EXPECTED_SAVE_FILE_BYTES {
        return Err(SaveSourceRejection::WrongSize {
            len: meta.len(),
            expected: EXPECTED_SAVE_FILE_BYTES,
        });
    }
    let bytes = std::fs::read(&path).map_err(|_| SaveSourceRejection::Unreadable)?;
    er_save_loader::bnd4::parse_entries(&bytes).map_err(|_| SaveSourceRejection::NotBnd4)?;
    Ok(path)
}

/// Convert a configured path root to the Wine drive form the in-process `CreateFileW` accepts.
/// Unix absolute paths become `Z:\...`; already-Windows/Wine paths like `Z:\...` or `C:\...`
/// are preserved. Backslash separators, no trailing separator. Returns UTF-16 without a NUL.
pub fn path_root_to_wine_wide(root: &Path) -> WineRootWide {
    let win: String = root
        // UTF-8 Lossy: OS path display/Win32 path bridge only; invalid host bytes are still mapped
        // into a deterministic Wine path string rather than decoded from game memory.
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
    WineRootWide(out)
}

pub fn plausible_steam_id64(value: u64) -> Option<u64> {
    (value >= 10_000_000_000_000_000 && value <= 99_999_999_999_999_999).then_some(value)
}

/// Save-like wide path category used by save-redirect telemetry and hook decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePathKind {
    None,
    EldenRingRoot,
    GraphicsConfig,
    StageSteamIdDir,
    StageSaveFile,
    ConfiguredSaveFile,
    OtherSaveLike,
}

impl SavePathKind {
    pub const fn as_usize(self) -> usize {
        match self {
            Self::None => 0,
            Self::EldenRingRoot => 1,
            Self::GraphicsConfig => 2,
            Self::StageSteamIdDir => 3,
            Self::StageSaveFile => 4,
            Self::ConfiguredSaveFile => 5,
            Self::OtherSaveLike => 6,
        }
    }

    pub const fn from_usize(value: usize) -> Self {
        match value {
            1 => Self::EldenRingRoot,
            2 => Self::GraphicsConfig,
            3 => Self::StageSteamIdDir,
            4 => Self::StageSaveFile,
            5 => Self::ConfiguredSaveFile,
            6 => Self::OtherSaveLike,
            _ => Self::None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::EldenRingRoot => "eldenring_root",
            Self::GraphicsConfig => "graphics_config",
            Self::StageSteamIdDir => "stage_steamid_dir",
            Self::StageSaveFile => "stage_save_file",
            Self::ConfiguredSaveFile => "configured_save_file",
            Self::OtherSaveLike => "other_save_like",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectStageNoSteamIdKind {
    None,
    EldenRingRoot,
    GraphicsConfig,
    ConfiguredSave,
    Other,
}

impl DirectStageNoSteamIdKind {
    pub const fn as_usize(self) -> usize {
        match self {
            Self::None => 0,
            Self::EldenRingRoot => 1,
            Self::GraphicsConfig => 2,
            Self::ConfiguredSave => 3,
            Self::Other => 4,
        }
    }

    pub const fn from_usize(value: usize) -> Self {
        match value {
            1 => Self::EldenRingRoot,
            2 => Self::GraphicsConfig,
            3 => Self::ConfiguredSave,
            4 => Self::Other,
            _ => Self::None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::EldenRingRoot => "eldenring_root",
            Self::GraphicsConfig => "graphics_config",
            Self::ConfiguredSave => "configured_save_without_steamid",
            Self::Other => "other",
            Self::None => "none",
        }
    }
}

/// ASCII-lowercase a UTF-16 code unit (leaves non-ASCII untouched).
pub fn wide_ascii_lower(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

/// True if `hay` contains `needle` (ASCII, case-insensitive). `needle` must be ASCII lowercase.
pub fn wide_contains_ci_ascii(hay: &[u16], needle: &[u16]) -> bool {
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
pub fn wide_find_ci_ascii(hay: &[u16], needle: &[u16]) -> Option<usize> {
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
pub fn wide_ends_with_ci_ascii(hay: &[u16], suffix: &[u16]) -> bool {
    if suffix.len() > hay.len() {
        return false;
    }
    let start = hay.len() - suffix.len();
    suffix
        .iter()
        .enumerate()
        .all(|(i, &s)| wide_ascii_lower(hay[start + i]) == s)
}

pub fn steam_id64_from_wide_save_path(path: &[u16]) -> Option<u64> {
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

fn is_primary_save_file_path(path: &[u16]) -> bool {
    const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
    const CO2D: &[u16] = &[b'.' as u16, b'c' as u16, b'o' as u16, b'2' as u16];
    wide_ends_with_ci_ascii(path, SL2D) || wide_ends_with_ci_ascii(path, CO2D)
}

pub fn is_save_file_or_backup_path(path: &[u16]) -> bool {
    const BAKD: &[u16] = &[b'.' as u16, b'b' as u16, b'a' as u16, b'k' as u16];
    is_primary_save_file_path(path) || wide_ends_with_ci_ascii(path, BAKD)
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

pub fn direct_stage_no_steamid_kind(path: &[u16]) -> DirectStageNoSteamIdKind {
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
        DirectStageNoSteamIdKind::GraphicsConfig
    } else if wide_ends_with_ci_ascii(path, SL2D) || wide_ends_with_ci_ascii(path, CO2D) {
        DirectStageNoSteamIdKind::ConfiguredSave
    } else if wide_ends_with_separator_or_eldenring(path) {
        DirectStageNoSteamIdKind::EldenRingRoot
    } else {
        DirectStageNoSteamIdKind::Other
    }
}

pub fn classify_save_like_path(path: &[u16]) -> SavePathKind {
    match steam_id64_from_wide_save_path(path) {
        Some(_) if is_primary_save_file_path(path) => SavePathKind::StageSaveFile,
        Some(_) => SavePathKind::StageSteamIdDir,
        None => match direct_stage_no_steamid_kind(path) {
            DirectStageNoSteamIdKind::ConfiguredSave => SavePathKind::ConfiguredSaveFile,
            DirectStageNoSteamIdKind::GraphicsConfig => SavePathKind::GraphicsConfig,
            DirectStageNoSteamIdKind::EldenRingRoot => SavePathKind::EldenRingRoot,
            _ => SavePathKind::OtherSaveLike,
        },
    }
}

/// Redirect a Windows/Wine wide path rooted under `%APPDATA%\\Roaming\\EldenRing` to a staged
/// save root. Returns a NUL-terminated wide path.
///
/// The `Roaming` anchor prevents already-redirected staged paths from being redirected again. The
/// `EldenRing` suffix is lowercased because the staged tree is created on a case-sensitive Linux
/// filesystem as lowercase `eldenring/<steamid>/er0000.*`.
pub fn redirect_wide_roaming_eldenring_path(path: &[u16], root_wide: &[u16]) -> Option<Vec<u16>> {
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
    if !wide_contains_ci_ascii(path, ROAMING) {
        return None;
    }
    let idx = wide_find_ci_ascii(path, ELDENRING)?;
    let suffix = &path[idx..];
    let mut out = Vec::with_capacity(root_wide.len() + 1 + suffix.len() + 1);
    out.extend_from_slice(root_wide);
    out.push(b'\\' as u16);
    for &c in suffix {
        out.push(wide_ascii_lower(c));
    }
    out.push(0);
    Some(out)
}

/// Shared save-path redirect flow with product-owned side effects injected at the boundary.
///
/// `observe_path` always runs first so the product can learn the active SteamID even when no redirect
/// root is active. `ensure_staged_path` runs only after the path is known to redirect, preserving the
/// old behavior that staging does not fire for non-Roaming/non-EldenRing paths or missing roots.
pub fn redirect_wide_save_path_with_side_effects(
    path: &[u16],
    root_wide: Option<&[u16]>,
    observe_path: impl FnOnce(&[u16]),
    ensure_staged_path: impl FnOnce(&[u16]),
) -> Option<Vec<u16>> {
    observe_path(path);
    let root_wide = root_wide?;
    let redirected = redirect_wide_roaming_eldenring_path(path, root_wide)?;
    ensure_staged_path(path);
    Some(redirected)
}

/// If `path` is under `<root>/EldenRing/<steamid>/...`, return that root plus steam id.
pub fn staged_save_root_for_file(path: &Path) -> Option<(PathBuf, u64)> {
    let mut root = PathBuf::new();
    let mut comps = path.components().peekable();
    while let Some(comp) = comps.next() {
        // UTF-8 Lossy: path component classification only; invalid host bytes cannot be a literal
        // `EldenRing` directory name and should fail the staged-root shortcut deterministically.
        let text = comp.as_os_str().to_string_lossy();
        if text.eq_ignore_ascii_case("EldenRing") {
            let Some(steam_id_comp) = comps.peek() else {
                return None;
            };
            // UTF-8 Lossy: SteamID directory classification only; invalid host bytes are rejected by
            // the ASCII-digit check below.
            let steam_id = steam_id_comp.as_os_str().to_string_lossy();
            let is_steam_id = (16..=20).contains(&steam_id.len())
                && steam_id.as_bytes().iter().all(u8::is_ascii_digit);
            if is_steam_id {
                return steam_id
                    .parse::<u64>()
                    .ok()
                    .and_then(plausible_steam_id64)
                    .map(|value| (root, value));
            }
            return None;
        }
        root.push(comp);
    }
    None
}

/// Build the redirect source plan for an already validated save file.
pub fn plan_validated_save_source(path: PathBuf, writeback_allowed: bool) -> SaveSourcePlan {
    if writeback_allowed && let Some((staged_root, steam_id)) = staged_save_root_for_file(&path) {
        return SaveSourcePlan::StagedRoot {
            file: path,
            steam_id,
            root_wide: path_root_to_wine_wide(&staged_root),
        };
    }

    let stage_root = path
        .parent()
        .map(|parent| parent.join("er-effects-save-redirect-stage"))
        .unwrap_or_else(|| PathBuf::from("er-effects-save-redirect-stage"));
    SaveSourcePlan::DirectFile {
        file: path.clone(),
        root_wide: path_root_to_wine_wide(&stage_root),
        stage_root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_LEN: usize = 0x40;
    const ENTRY_STRIDE: usize = 0x20;
    const MD5_LEN: usize = 0x10;

    fn synthetic_bnd4_container() -> Vec<u8> {
        let body = vec![0_u8; 0x20];
        let name = "USER_DATA010";
        let mut name_blob: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        name_blob.extend_from_slice(&[0, 0]);
        let names_at = HEADER_LEN + ENTRY_STRIDE;
        let data_at = names_at + name_blob.len();
        let total = EXPECTED_SAVE_FILE_BYTES as usize;
        let mut out = vec![0_u8; total];
        out[..4].copy_from_slice(b"BND4");
        out[0x0c..0x10].copy_from_slice(&1_i32.to_le_bytes());
        out[0x10..0x18].copy_from_slice(&(HEADER_LEN as i64).to_le_bytes());
        out[0x20..0x28].copy_from_slice(&(ENTRY_STRIDE as i64).to_le_bytes());
        out[HEADER_LEN + 0x08..HEADER_LEN + 0x10]
            .copy_from_slice(&((MD5_LEN + body.len()) as i64).to_le_bytes());
        out[HEADER_LEN + 0x10..HEADER_LEN + 0x14].copy_from_slice(&(data_at as i32).to_le_bytes());
        out[HEADER_LEN + 0x14..HEADER_LEN + 0x18].copy_from_slice(&(names_at as i32).to_le_bytes());
        out[names_at..names_at + name_blob.len()].copy_from_slice(&name_blob);
        out[data_at + MD5_LEN..data_at + MD5_LEN + body.len()].copy_from_slice(&body);
        out
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("er-save-redirect-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
        dir
    }

    #[test]
    fn missing_save_gate_moves_only_through_explicit_states() {
        let gate = MissingSaveGate::new();
        assert_eq!(gate.state(), MissingSaveState::Idle);
        assert!(!gate.is_pending());
        gate.set(MissingSaveState::Pending);
        assert!(gate.is_pending());
        gate.set(MissingSaveState::Ready);
        assert_eq!(gate.state(), MissingSaveState::Ready);
    }

    #[test]
    fn save_hook_install_state_runs_each_install_gate_once() {
        let state = SaveHookInstallState::new();
        let core_calls = std::cell::Cell::new(0);
        state.install_core_once(|| core_calls.set(core_calls.get() + 1));
        state.install_core_once(|| core_calls.set(core_calls.get() + 1));
        assert_eq!(core_calls.get(), 1);
        assert!(!state.core_createfilew_installed());
        state.mark_core_createfilew_installed();
        assert!(state.core_createfilew_installed());

        let redirect_calls = std::cell::Cell::new(0);
        state.install_redirect_once(|| redirect_calls.set(redirect_calls.get() + 1));
        state.install_redirect_once(|| redirect_calls.set(redirect_calls.get() + 1));
        assert_eq!(redirect_calls.get(), 1);
    }

    #[test]
    fn classifies_shgetfolderpath_appdata_redirects() {
        assert!(shgetfolderpath_is_appdata_request(SHGFP_CSIDL_APPDATA));
        assert!(shgetfolderpath_is_appdata_request(
            0x8000 | SHGFP_CSIDL_APPDATA
        ));
        assert!(!shgetfolderpath_is_appdata_request(0x20));
        assert_eq!(
            shgetfolderpath_staged_appdata_len(SHGFP_CSIDL_APPDATA, false, Some(400), 259),
            Some(259)
        );
        assert_eq!(
            shgetfolderpath_staged_appdata_len(SHGFP_CSIDL_APPDATA, true, Some(10), 259),
            None
        );
        assert_eq!(
            shgetfolderpath_staged_appdata_len(SHGFP_CSIDL_APPDATA, false, None, 259),
            None
        );
    }

    #[test]
    fn writes_shgetfolderpath_staged_root_with_nul() {
        let root = wide_path(r"Z:\stage");
        let mut out = vec![0xffff; root.len() + 2];
        let copied = unsafe { write_shgetfolderpath_staged_root(out.as_mut_ptr(), &root, 4) };
        assert_eq!(copied, 4);
        assert_eq!(
            &out[..5],
            &[b'Z' as u16, b':' as u16, b'\\' as u16, b's' as u16, 0]
        );
    }

    #[test]
    fn classifies_createfile_save_paths_for_callbacks() {
        let save = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.co2");
        let diag = classify_create_file_save_path(&save);
        assert!(diag.save_like);
        assert!(diag.is_save_file);
        assert!(diag.should_wait_for_missing_save_dialog);
        assert!(diag.should_capture_diag_log(99));

        let backup = wide_path(r"Z:\stage\EldenRing\76561197960265729\ER0000.sl2.bak");
        let backup_diag = classify_create_file_save_path(&backup);
        assert!(backup_diag.save_like);
        assert!(!backup_diag.is_save_file);
        assert!(backup_diag.should_wait_for_missing_save_dialog);

        let graphics = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\GraphicsConfig.xml");
        let graphics_diag = classify_create_file_save_path(&graphics);
        assert!(graphics_diag.save_like);
        assert!(!graphics_diag.is_save_file);
        assert!(!graphics_diag.should_wait_for_missing_save_dialog);
        assert!(!classify_create_file_save_path(&wide_path(r"C:\tmp\notes.txt")).save_like);
    }

    #[test]
    fn createfile_diag_hit_logging_keeps_first_eight_and_powers_of_two() {
        assert!((1..=8).all(createfile_diag_hit_should_log));
        assert!(!createfile_diag_hit_should_log(9));
        assert!(!createfile_diag_hit_should_log(31));
        assert!(createfile_diag_hit_should_log(32));
    }

    #[test]
    fn classifies_ntcreatefile_save_paths_for_callbacks() {
        let read = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2");
        let diag = classify_nt_create_file_save_path(&read, 0);
        assert_eq!(
            diag,
            NtCreateFileSavePathDiag {
                is_save_file_or_backup: true,
                is_sl2: true,
                is_write: false,
            }
        );
        assert!(diag.should_wait_for_missing_save_dialog());
        assert!(diag.should_observe_steam_id());
        assert!(diag.should_normalize_on_read());
        assert!(diag.should_capture_diag_log(7, 8));
        assert!(!diag.should_capture_diag_log(8, 8));

        let write = classify_nt_create_file_save_path(&read, NT_CREATEFILE_GENERIC_WRITE);
        assert!(write.is_write);
        assert!(!write.should_normalize_on_read());

        let backup =
            wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2.bak");
        let backup_diag = classify_nt_create_file_save_path(&backup, NT_CREATEFILE_FILE_WRITE_DATA);
        assert!(backup_diag.is_save_file_or_backup);
        assert!(!backup_diag.is_sl2);
        assert!(backup_diag.is_write);
    }

    #[test]
    fn ntcreatefile_diag_hit_logging_keeps_first_eight_and_powers_of_two() {
        assert!((1..=8).all(nt_createfile_diag_hit_should_log));
        assert!(!nt_createfile_diag_hit_should_log(9));
        assert!(!nt_createfile_diag_hit_should_log(15));
        assert!(nt_createfile_diag_hit_should_log(16));
    }

    #[test]
    fn fills_get_disk_free_space_outputs_with_ample_bytes() {
        let mut free = 1;
        let mut total = 2;
        let mut total_free = 3;
        unsafe {
            fill_get_disk_free_space_ex_outputs(&mut free, std::ptr::null_mut(), &mut total_free);
        }
        assert_eq!(free, SAVE_REDIRECT_AMPLE_FREE_BYTES);
        assert_eq!(total, 2);
        assert_eq!(total_free, SAVE_REDIRECT_AMPLE_FREE_BYTES);

        unsafe {
            fill_get_disk_free_space_ex_outputs(
                std::ptr::null_mut(),
                &mut total,
                std::ptr::null_mut(),
            );
        }
        assert_eq!(total, SAVE_REDIRECT_AMPLE_FREE_BYTES);
    }

    #[test]
    fn patches_ntquery_volume_free_space_outputs() {
        let mut size_info = [10_i64, 20_i64, 30_i64];
        let ptr = size_info.as_mut_ptr() as *mut u8;
        assert_eq!(
            unsafe { ntquery_volume_available_units(0, ptr, 16, FILE_FS_SIZE_INFORMATION_CLASS) },
            Some(20)
        );
        assert!(unsafe {
            patch_ntquery_volume_free_space(0, ptr, 16, FILE_FS_SIZE_INFORMATION_CLASS)
        });
        assert_eq!(
            &size_info,
            &[
                SAVE_REDIRECT_AMPLE_FREE_UNITS,
                SAVE_REDIRECT_AMPLE_FREE_UNITS,
                30,
            ]
        );

        let mut full_info = [10_i64, 20_i64, 30_i64];
        let ptr = full_info.as_mut_ptr() as *mut u8;
        assert!(unsafe {
            patch_ntquery_volume_free_space(0, ptr, 24, FILE_FS_FULL_SIZE_INFORMATION_CLASS)
        });
        assert_eq!(full_info, [SAVE_REDIRECT_AMPLE_FREE_UNITS; 3]);

        assert!(!unsafe {
            patch_ntquery_volume_free_space(0, ptr, 8, FILE_FS_SIZE_INFORMATION_CLASS)
        });
        assert!(!unsafe {
            patch_ntquery_volume_free_space(-1, ptr, 24, FILE_FS_FULL_SIZE_INFORMATION_CLASS)
        });
        assert_eq!(
            unsafe { ntquery_volume_available_units(0, ptr, 16, 1) },
            None
        );
    }

    fn wide_path(path: &str) -> Vec<u16> {
        path.encode_utf16().collect()
    }

    #[test]
    fn classifies_wide_save_paths_for_hook_telemetry() {
        let stage_file =
            wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2");
        assert_eq!(
            steam_id64_from_wide_save_path(&stage_file),
            Some(76_561_197_960_265_729)
        );
        assert_eq!(
            classify_save_like_path(&stage_file),
            SavePathKind::StageSaveFile
        );

        let stage_dir = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\");
        assert_eq!(
            classify_save_like_path(&stage_dir),
            SavePathKind::StageSteamIdDir
        );
        let backup =
            wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2.bak");
        assert!(is_save_file_or_backup_path(&backup));
        assert_eq!(
            classify_save_like_path(&backup),
            SavePathKind::StageSteamIdDir
        );

        let graphics = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\GraphicsConfig.xml");
        assert_eq!(
            direct_stage_no_steamid_kind(&graphics),
            DirectStageNoSteamIdKind::GraphicsConfig
        );
        assert_eq!(
            classify_save_like_path(&graphics),
            SavePathKind::GraphicsConfig
        );

        let root = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\\");
        assert_eq!(
            direct_stage_no_steamid_kind(&root),
            DirectStageNoSteamIdKind::EldenRingRoot
        );
        assert_eq!(classify_save_like_path(&root), SavePathKind::EldenRingRoot);

        let loose_save = wide_path(r"Z:\tmp\picked\ER0000.co2");
        assert_eq!(
            direct_stage_no_steamid_kind(&loose_save),
            DirectStageNoSteamIdKind::ConfiguredSave
        );
        assert_eq!(
            classify_save_like_path(&loose_save),
            SavePathKind::ConfiguredSaveFile
        );
    }

    #[test]
    fn redirects_roaming_eldenring_paths_to_staged_root() {
        let root = wide_path(r"Z:\tmp\stage");
        let source =
            wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.SL2");
        let redirected = redirect_wide_roaming_eldenring_path(&source, &root).unwrap();
        assert_eq!(
            String::from_utf16(&redirected[..redirected.len() - 1]).unwrap(),
            r"Z:\tmp\stage\eldenring\76561197960265729\er0000.sl2"
        );
        assert_eq!(redirected.last(), Some(&0));

        let already_staged = wide_path(r"Z:\tmp\stage\EldenRing\76561197960265729\ER0000.sl2");
        assert_eq!(
            redirect_wide_roaming_eldenring_path(&already_staged, &root),
            None
        );
    }

    #[test]
    fn redirect_flow_preserves_observe_then_stage_side_effect_order() {
        let root = wide_path(r"Z:\tmp\stage");
        let source =
            wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2");
        let events = std::cell::RefCell::new(Vec::new());
        let redirected = redirect_wide_save_path_with_side_effects(
            &source,
            Some(&root),
            |_| events.borrow_mut().push("observe"),
            |_| events.borrow_mut().push("ensure"),
        )
        .unwrap();
        assert_eq!(&*events.borrow(), &["observe", "ensure"]);
        assert_eq!(redirected.last(), Some(&0));
    }

    #[test]
    fn redirect_flow_observes_but_does_not_stage_without_a_redirect() {
        let root = wide_path(r"Z:\tmp\stage");
        let non_save = wide_path(r"C:\Users\x\Desktop\EldenRing\ER0000.sl2");
        let events = std::cell::RefCell::new(Vec::new());
        assert_eq!(
            redirect_wide_save_path_with_side_effects(
                &non_save,
                Some(&root),
                |_| events.borrow_mut().push("observe"),
                |_| events.borrow_mut().push("ensure"),
            ),
            None
        );
        assert_eq!(&*events.borrow(), &["observe"]);

        events.borrow_mut().clear();
        assert_eq!(
            redirect_wide_save_path_with_side_effects(
                &non_save,
                None,
                |_| events.borrow_mut().push("observe"),
                |_| events.borrow_mut().push("ensure"),
            ),
            None
        );
        assert_eq!(&*events.borrow(), &["observe"]);
    }

    #[test]
    fn validation_rejects_wrong_size_or_non_bnd4_files() {
        let dir = scratch_dir("rejects");
        let tiny = dir.join("ER0000.sl2");
        std::fs::write(&tiny, b"BND4").unwrap();
        assert_eq!(
            validate_save_file_path(tiny),
            Err(SaveSourceRejection::WrongSize {
                len: 4,
                expected: EXPECTED_SAVE_FILE_BYTES,
            })
        );

        let garbage = dir.join("large.sl2");
        std::fs::write(&garbage, vec![0_u8; EXPECTED_SAVE_FILE_BYTES as usize]).unwrap();
        assert_eq!(
            validate_save_file_path(garbage),
            Err(SaveSourceRejection::NotBnd4)
        );
    }

    #[test]
    fn validation_accepts_a_structural_bnd4_container() {
        let dir = scratch_dir("accepts");
        let save = dir.join("ER0000.sl2");
        std::fs::write(&save, synthetic_bnd4_container()).unwrap();
        assert_eq!(validate_save_file_path(save.clone()), Ok(save));
    }

    #[test]
    fn staged_root_plan_uses_the_ancestor_before_eldenring() {
        let path = PathBuf::from("Z:/prefix/EldenRing/76561198000000000/ER0000.sl2");
        let plan = plan_validated_save_source(path.clone(), true);
        assert_eq!(
            plan,
            SaveSourcePlan::StagedRoot {
                file: path,
                steam_id: 76_561_198_000_000_000,
                root_wide: WineRootWide("Z:\\prefix".encode_utf16().collect()),
            }
        );
    }

    #[test]
    fn arbitrary_save_files_plan_a_private_stage_root() {
        let path = PathBuf::from("/tmp/picked/ER0000.sl2");
        let plan = plan_validated_save_source(path.clone(), true);
        assert_eq!(
            plan,
            SaveSourcePlan::DirectFile {
                file: path,
                stage_root: PathBuf::from("/tmp/picked/er-effects-save-redirect-stage"),
                root_wide: WineRootWide(
                    "Z:\\tmp\\picked\\er-effects-save-redirect-stage"
                        .encode_utf16()
                        .collect(),
                ),
            }
        );
    }
}
