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
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

/// Exact byte length of Elden Ring PC `ER0000.sl2` / Seamless `.co2` save containers.
///
/// These files use a fixed BND4 layout: ten `USER_DATA00N` character slots, `USER_DATA010`, and
/// `USER_DATA011`. A different length is not a valid Elden Ring save container for this loader.
pub const EXPECTED_SAVE_FILE_BYTES: u64 = 0x1ba03d0;

/// Result of recording a deterministic failure in [`TerminalRejectionGuard`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalRejectionObservation {
    First,
    RepeatedIdentical,
    RepeatedDifferent,
}

/// Fail-closed process-local latch for a rejection that cannot become resolvable without an explicit
/// state transition.
///
/// The first rejection publishes a nonzero fingerprint and consumes the attempt. Any caller that
/// reaches the same resolver again can record the attempt: an identical fingerprint increments the
/// recurrence semaphore instead of disguising a deterministic failure as another retry.
pub struct TerminalRejectionGuard {
    fingerprint: AtomicU64,
    attempts: AtomicU64,
    repeated_identical: AtomicU64,
    repeated_different: AtomicU64,
}

impl TerminalRejectionGuard {
    pub const fn new() -> Self {
        Self {
            fingerprint: AtomicU64::new(0),
            attempts: AtomicU64::new(0),
            repeated_identical: AtomicU64::new(0),
            repeated_different: AtomicU64::new(0),
        }
    }

    pub fn record(&self, fingerprint: u64) -> TerminalRejectionObservation {
        // Zero means "no rejection" in telemetry. Preserve that invariant even for the vanishingly
        // unlikely input whose hash is zero.
        let fingerprint = fingerprint.max(1);
        self.attempts.fetch_add(1, Ordering::SeqCst);
        match self
            .fingerprint
            .compare_exchange(0, fingerprint, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => TerminalRejectionObservation::First,
            Err(current) if current == fingerprint => {
                self.repeated_identical.fetch_add(1, Ordering::SeqCst);
                TerminalRejectionObservation::RepeatedIdentical
            }
            Err(_) => {
                self.repeated_different.fetch_add(1, Ordering::SeqCst);
                TerminalRejectionObservation::RepeatedDifferent
            }
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.fingerprint() != 0
    }

    pub fn fingerprint(&self) -> u64 {
        self.fingerprint.load(Ordering::SeqCst)
    }

    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::SeqCst)
    }

    pub fn repeated_identical(&self) -> u64 {
        self.repeated_identical.load(Ordering::SeqCst)
    }

    pub fn repeated_different(&self) -> u64 {
        self.repeated_different.load(Ordering::SeqCst)
    }
}

impl Default for TerminalRejectionGuard {
    fn default() -> Self {
        Self::new()
    }
}

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

/// Whether the redirect-mode save hook batch should install now.
///
/// The native missing-save picker path deliberately keeps redirect hooks uninstalled until a picked
/// or configured redirect root exists. Trace mode is the diagnostic exception: it installs the batch
/// without a redirect root so hook observations can be collected.
fn redirect_save_hooks_install_ready(redirect_root_ready: bool, trace_enabled: bool) -> bool {
    redirect_root_ready || trace_enabled
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
                // The detour must outlive this scope for the process lifetime; the `forget` states
                // that even though `MhHook` is currently plain pointers, so a future `Drop` that
                // unhooks cannot silently retire the save-redirect CreateFileW hook.
                #[allow(
                    clippy::forget_non_drop,
                    reason = "intent marker: the installed detour must never be released"
                )]
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

/// Install the redirect-mode save hook batch only after a redirect root or trace mode is ready.
///
/// This keeps the native no-save picker boot path unhooked until the product has activated a picked
/// source, while still allowing explicit trace mode to install the observation hooks.
///
/// # Safety
/// Same requirements as [`install_redirect_save_hooks`].
#[allow(
    clippy::too_many_arguments,
    reason = "readiness gate in front of `install_redirect_save_hooks`; it forwards that function's argument list unchanged"
)]
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
    if !redirect_save_hooks_install_ready(redirect_root_ready, trace_enabled) {
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
#[allow(
    clippy::too_many_arguments,
    reason = "dependency-injection seam: each export resolver, detour bundle and sink is a distinct collaborator supplied by the product DLL"
)]
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

/// Shared per-endpoint decision for `CopyFileW` save redirects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFileEndpointRedirect {
    pub should_wait_for_missing_save_dialog: bool,
    pub redirected: Option<Vec<u16>>,
}

pub fn classify_copyfile_endpoint(
    path: &[u16],
    redirect_path: impl FnOnce(&[u16]) -> Option<Vec<u16>>,
) -> CopyFileEndpointRedirect {
    CopyFileEndpointRedirect {
        should_wait_for_missing_save_dialog: is_save_file_or_backup_path(path),
        redirected: redirect_path(path),
    }
}

/// Shared classification for save existence/query APIs (`GetFileAttributes*`, `FindFirstFileW`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveQueryPathDiag {
    pub contains_eldenring: bool,
    pub contains_er0000: bool,
}

impl SaveQueryPathDiag {
    pub fn should_record_path_kind(self) -> bool {
        self.contains_eldenring || self.contains_er0000
    }

    pub fn should_capture_save_file_query_log(self, logged: usize, max: usize) -> bool {
        self.contains_er0000 && logged < max
    }

    pub fn should_capture_general_query_log(self, logged: usize, max: usize) -> bool {
        self.contains_eldenring && logged < max
    }
}

pub fn classify_save_query_path(path: &[u16]) -> SaveQueryPathDiag {
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
    const ER0000: &[u16] = &[
        b'e' as u16,
        b'r' as u16,
        b'0' as u16,
        b'0' as u16,
        b'0' as u16,
        b'0' as u16,
    ];
    SaveQueryPathDiag {
        contains_eldenring: wide_contains_ci_ascii(path, ELDENRING_SEG),
        contains_er0000: wide_contains_ci_ascii(path, ER0000),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveQueryPathPlan {
    pub diag: SaveQueryPathDiag,
    pub redirected: Option<Vec<u16>>,
}

pub fn plan_save_query_path(
    path: &[u16],
    redirect_path: impl FnOnce(&[u16]) -> Option<Vec<u16>>,
) -> SaveQueryPathPlan {
    SaveQueryPathPlan {
        diag: classify_save_query_path(path),
        redirected: redirect_path(path),
    }
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

/// Shared post-save-destination CreateFileW open plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFileOpenPlan {
    pub diag: CreateFileSavePathDiag,
    pub redirected: Option<Vec<u16>>,
}

impl CreateFileOpenPlan {
    pub fn should_wait_for_missing_save_dialog(&self) -> bool {
        self.diag.should_wait_for_missing_save_dialog
    }

    pub fn should_normalize_on_save_open(&self) -> bool {
        self.diag.is_save_file
    }
}

pub fn plan_create_file_open(
    path: &[u16],
    redirect_path: impl FnOnce(&[u16]) -> Option<Vec<u16>>,
) -> CreateFileOpenPlan {
    CreateFileOpenPlan {
        diag: classify_create_file_save_path(path),
        redirected: redirect_path(path),
    }
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
    (10_000_000_000_000_000..=99_999_999_999_999_999)
        .contains(&value)
        .then_some(value)
}

pub fn steam_id64_from_dir_name(name: &str) -> Option<u64> {
    let is_steam_id =
        (16..=20).contains(&name.len()) && name.as_bytes().iter().all(u8::is_ascii_digit);
    is_steam_id
        .then(|| name.parse::<u64>().ok())
        .flatten()
        .and_then(plausible_steam_id64)
}

pub fn default_save_file_path(root: &Path, steam_id: u64, file_name: &str) -> PathBuf {
    root.join(steam_id.to_string()).join(file_name)
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

    pub const fn telemetry_bucket(self) -> Option<SavePathTelemetryBucket> {
        match self {
            Self::StageSteamIdDir => Some(SavePathTelemetryBucket::StageSteamIdDir),
            Self::StageSaveFile => Some(SavePathTelemetryBucket::StageSaveFile),
            Self::ConfiguredSaveFile => Some(SavePathTelemetryBucket::ConfiguredSaveFile),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePathTelemetryBucket {
    StageSteamIdDir,
    StageSaveFile,
    ConfiguredSaveFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavePathTelemetryPlan {
    pub kind: SavePathKind,
    pub bucket: Option<SavePathTelemetryBucket>,
}

pub fn plan_save_path_telemetry(path: &[u16]) -> SavePathTelemetryPlan {
    let kind = classify_save_like_path(path);
    SavePathTelemetryPlan {
        kind,
        bucket: kind.telemetry_bucket(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectStageRequestPlan {
    SteamId(u64),
    NoSteamId(DirectStageNoSteamIdKind),
}

pub fn plan_direct_stage_request(path: &[u16]) -> DirectStageRequestPlan {
    match steam_id64_from_wide_save_path(path) {
        Some(steam_id) => DirectStageRequestPlan::SteamId(steam_id),
        None => DirectStageRequestPlan::NoSteamId(direct_stage_no_steamid_kind(path)),
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
            let steam_id_comp = comps.peek()?;
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
pub fn path_eq_ignore_ascii_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(
            b.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\'),
        )
}

pub fn save_file_writeback_allowed(path: &Path, default_root: Option<&Path>) -> bool {
    let Some(default_root) = default_root else {
        return false;
    };
    path.parent()
        .and_then(|steam_dir| steam_dir.parent())
        .is_some_and(|root| path_eq_ignore_ascii_case(root, default_root))
}

pub fn save_file_is_readonly(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().readonly())
}

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
        .map(|parent| parent.join(DIRECT_STAGE_ROOT_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(DIRECT_STAGE_ROOT_DIR_NAME));
    SaveSourcePlan::DirectFile {
        file: path.clone(),
        root_wide: path_root_to_wine_wide(&stage_root),
        stage_root,
    }
}

/// Directory name of the private staged save tree a DIRECT-FILE source is copied into.
///
/// The staleness sweep only ever deletes inside a directory carrying this component, so the
/// constant is the containment proof as much as it is the path builder.
pub const DIRECT_STAGE_ROOT_DIR_NAME: &str = "er-effects-save-redirect-stage";

pub fn direct_stage_case_dirs(root: &Path) -> [PathBuf; 2] {
    [root.join("eldenring"), root.join("EldenRing")]
}

/// True when `dir` lies inside a private staged save tree (a `DIRECT_STAGE_ROOT_DIR_NAME`
/// component appears somewhere in it).
///
/// Staging deletes stale containers, and a delete is only ever safe inside our own tree: the
/// configured source itself lives one level ABOVE the stage root and is read-only by contract.
pub fn is_inside_direct_stage_root(dir: &Path) -> bool {
    dir.components().any(|comp| {
        // UTF-8 Lossy: path component classification only; invalid host bytes cannot spell the
        // stage-root directory name and must fail this containment check deterministically.
        comp.as_os_str().to_string_lossy() == DIRECT_STAGE_ROOT_DIR_NAME
    })
}

/// The vanilla save container. Elden Ring itself writes only this one.
pub const VANILLA_SAVE_CONTAINER_NAME: &str = "ER0000.sl2";
/// The extension ERSC ships with in `ersc_settings.ini`. It is a DEFAULT, never an invariant --
/// see [`parse_ersc_save_file_extension`].
pub const DEFAULT_SEAMLESS_SAVE_FILE_EXTENSION: &str = "co2";
/// ERSC's own documented ceiling for `save_file_extension` ("limit = 120").
pub const MAX_SAVE_FILE_EXTENSION_LEN: usize = 120;

/// The Seamless save-container extension ERSC is CONFIGURED with, from its `ersc_settings.ini`.
///
/// `.co2` is only the shipped default. `ersc_settings.ini` says, in its own words: "Your save file
/// extension (in the vanilla game this is .sl2). Use any alphanumeric characters (limit = 120)" --
/// so the value REPLACES `sl2` and a user may set it to anything. Every hard-coded `.co2` in a save
/// path is therefore a latent version of the same bug this module exists to fix: the staged copy
/// carrying a name the runtime never asks for.
///
/// Returns None when the key is absent, outside `[SAVE]`, empty, over-long, or not plain ASCII
/// alphanumeric -- the last of which also keeps a config value from steering the staged filename
/// out of its directory.
pub fn parse_ersc_save_file_extension(ini: &str) -> Option<&str> {
    let mut in_save_section = false;
    for line in ini.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_save_section = line.eq_ignore_ascii_case("[SAVE]");
            continue;
        }
        if !in_save_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("save_file_extension") {
            continue;
        }
        let value = value.trim();
        let usable = (1..=MAX_SAVE_FILE_EXTENSION_LEN).contains(&value.len())
            && value.bytes().all(|b| b.is_ascii_alphanumeric());
        return usable.then_some(value);
    }
    None
}

/// The save container for one extension: `ER0000.<ext>`.
pub fn save_container_name_for_extension(extension: &str) -> String {
    format!("ER0000.{extension}")
}

/// Container names the active runtime will LOAD, in priority order.
///
/// The mode lock is ASYMMETRIC: Seamless takes both containers preferring the co-op one, vanilla
/// takes only `.sl2` so an offline launch can never advance co-op progress. `seamless_name` is the
/// co-op container ERSC is configured with, NOT a fixed `ER0000.co2`.
pub fn active_save_container_names_for(seamless: bool, seamless_name: &str) -> Vec<String> {
    if seamless && !seamless_name.eq_ignore_ascii_case(VANILLA_SAVE_CONTAINER_NAME) {
        vec![
            seamless_name.to_owned(),
            VANILLA_SAVE_CONTAINER_NAME.to_owned(),
        ]
    } else {
        vec![VANILLA_SAVE_CONTAINER_NAME.to_owned()]
    }
}

/// The container name the active runtime WRITES to -- the preferred load candidate.
pub fn active_save_container_name_for(seamless: bool, seamless_name: &str) -> String {
    if seamless {
        seamless_name.to_owned()
    } else {
        VANILLA_SAVE_CONTAINER_NAME.to_owned()
    }
}

/// Every container name a staging pass writes from the configured source.
///
/// BOTH the vanilla container and ERSC's configured one, always -- the staged name is derived
/// neither from the SOURCE file's extension nor from the Seamless mode. Measured 2026-08-11:
/// staging runs inside the `CreateFileW` detour at DllMain+191ms, and me3 loads `ersc.dll` after
/// that, so the ERSC module latch still reads `seamless=false` there (`save-picker mode from ERSC
/// module latch seamless=false reason=active-default-save-file-name`) while the same run's
/// telemetry later reports `seamless_coop_loaded=true`. Naming the staged copy from that unsettled
/// latch put a Seamless run's save at `ER0000.sl2` while `own_load::drive` and the native writer
/// asked for the co-op container, and the two never met -- a silent soft lock at the boot cover.
///
/// Writing both names removes the time-of-check race outright: whichever container the runtime
/// resolves to once the mode HAS settled, it holds the configured source. Restamping the name is
/// byte-safe -- every flavor is the same 28 MB BND4 container.
pub fn staged_save_container_names_for(seamless_name: &str) -> Vec<String> {
    let mut names = vec![VANILLA_SAVE_CONTAINER_NAME.to_owned()];
    if !seamless_name.eq_ignore_ascii_case(VANILLA_SAVE_CONTAINER_NAME) {
        names.push(seamless_name.to_owned());
    }
    names
}

/// True when `file_name` is one of the containers this staging pass rewrites.
pub fn is_staged_save_container_name(file_name: &str, staged_names: &[&str]) -> bool {
    staged_names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(file_name))
}

/// What happens to a file already sitting in a staged `<root>/<case>/<steamid>/` directory when a
/// new staging pass runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedEntryFate {
    /// A container this pass rewrites from the configured source, so its old bytes cannot survive.
    Rewritten,
    /// A save artifact left over from an EARLIER run that this pass does not rewrite: a `.bak`
    /// companion, a half-finished restore temp, a container under some other spelling. It does not
    /// correspond to the configured source and the game must never find it.
    StaleRemove,
    /// Not a save artifact (`GraphicsConfig.xml`, stray logs). Left alone.
    Keep,
}

/// Classify one existing staged directory entry against the containers this pass rewrites.
///
/// Nothing here consults mtimes. A staged file is current because THIS run wrote it from the
/// configured source, and stale otherwise -- the 2026-08-11 soft lock served a `.co2` written
/// 33 minutes earlier from a different source, and every timestamp involved looked plausible.
pub fn staged_entry_fate(file_name: &str, staged_names: &[&str]) -> StagedEntryFate {
    if is_staged_save_container_name(file_name, staged_names) {
        return StagedEntryFate::Rewritten;
    }
    let lower = file_name.to_ascii_lowercase();
    let save_artifact = lower.contains("er0000")
        || lower.ends_with(".sl2")
        || lower.ends_with(".co2")
        || lower.ends_with(".bak");
    if save_artifact {
        StagedEntryFate::StaleRemove
    } else {
        StagedEntryFate::Keep
    }
}

/// Drop paths that resolve to the same directory, keeping first-seen order.
///
/// The staged tree is created under both `eldenring` and `EldenRing` because a case-SENSITIVE host
/// filesystem needs both spellings. Under Wine those two resolve to one directory, so writing a
/// 28 MB container once per spelling doubles the DllMain staging cost for nothing. `identity` is
/// the caller's resolver (`fs::canonicalize` in product); a path it cannot resolve keeps its own
/// literal path as identity, so an unresolvable dir is never silently merged with another.
pub fn dedupe_dirs_by_identity(
    dirs: impl IntoIterator<Item = PathBuf>,
    identity: impl Fn(&Path) -> Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let key = identity(&dir).unwrap_or_else(|| dir.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(dir);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectStageFileStatus {
    pub exists: bool,
    pub bytes: Option<u64>,
}

/// Does the staged tree hold a container the runtime will actually LOAD?
///
/// `load_names` is the ACTIVE MODE's candidate list (`active_save_container_names`), never
/// anything derived from the source file's extension. Probing by source extension is what made
/// this oracle lie on 2026-08-11: it reported `direct_stage_file_exists=true` for a `.co2` source
/// while the run's staged copy had been written as `ER0000.sl2`, so the one telemetry field that
/// could have named the mismatch confirmed the staging instead.
pub fn probe_direct_stage_file_status(
    root: Option<&Path>,
    load_names: &[&str],
    steam_id: u64,
) -> DirectStageFileStatus {
    if steam_id == 0 {
        return DirectStageFileStatus {
            exists: false,
            bytes: None,
        };
    }
    let Some(root) = root else {
        return DirectStageFileStatus {
            exists: false,
            bytes: None,
        };
    };
    for file_name in load_names {
        for dir in direct_stage_case_dirs(root) {
            let path = dir.join(steam_id.to_string()).join(file_name);
            if let Ok(meta) = std::fs::metadata(path)
                && meta.is_file()
            {
                return DirectStageFileStatus {
                    exists: true,
                    bytes: Some(meta.len()),
                };
            }
        }
    }
    DirectStageFileStatus {
        exists: false,
        bytes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_LEN: usize = 0x40;
    const ENTRY_STRIDE: usize = 0x20;
    const MD5_LEN: usize = 0x10;

    #[test]
    fn terminal_rejection_simulation_resolves_once_across_recorded_runtime_churn() {
        let guard = TerminalRejectionGuard::new();
        let signature = er_game_base::fnv1a::fnv1a64(b"missing:ER0000.co2,ER0000.sl2");
        let mut resolver_calls = 0_u64;
        let mut fail_closed_ticks = 0_u64;

        // The preserved failure made 120,959 CreateFileW calls while the same save rejection was
        // reconsidered. Model that many driver ticks: only the first may reach the resolver.
        for _ in 0..120_959 {
            if guard.is_terminal() {
                fail_closed_ticks += 1;
                continue;
            }
            resolver_calls += 1;
            assert_eq!(guard.record(signature), TerminalRejectionObservation::First);
        }

        assert_eq!(resolver_calls, 1);
        assert_eq!(fail_closed_ticks, 120_958);
        assert!(guard.is_terminal());
        assert_ne!(guard.fingerprint(), 0);
        assert_eq!(guard.attempts(), 1);
        assert_eq!(guard.repeated_identical(), 0);
        assert_eq!(guard.repeated_different(), 0);
    }

    #[test]
    fn repeated_identical_rejection_sets_a_nonzero_recurrence_semaphore() {
        let guard = TerminalRejectionGuard::new();
        let signature = er_game_base::fnv1a::fnv1a64(b"missing:ER0000.sl2");
        assert_eq!(guard.record(signature), TerminalRejectionObservation::First);
        assert_eq!(
            guard.record(signature),
            TerminalRejectionObservation::RepeatedIdentical
        );
        assert_eq!(guard.attempts(), 2);
        assert_eq!(guard.repeated_identical(), 1);
        assert_eq!(guard.repeated_different(), 0);
    }

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
    fn no_runtime_hook_install_smoke_defers_until_ready_and_installs_once() {
        let state = SaveHookInstallState::new();
        let core_calls = std::cell::Cell::new(0);
        let redirect_calls = std::cell::Cell::new(0);
        let deferrals = std::cell::Cell::new(0);

        let attempt_install = |redirect_root_ready: bool, trace_enabled: bool| {
            if !redirect_save_hooks_install_ready(redirect_root_ready, trace_enabled) {
                deferrals.set(deferrals.get() + 1);
                return;
            }
            state.install_redirect_once(|| {
                redirect_calls.set(redirect_calls.get() + 1);
                state.install_core_once(|| core_calls.set(core_calls.get() + 1));
            });
        };

        attempt_install(false, false);
        assert_eq!(deferrals.get(), 1);
        assert_eq!(redirect_calls.get(), 0);
        assert_eq!(core_calls.get(), 0);

        attempt_install(false, true);
        assert_eq!(redirect_calls.get(), 1);
        assert_eq!(core_calls.get(), 1);

        attempt_install(true, false);
        attempt_install(false, true);
        state.install_core_once(|| core_calls.set(core_calls.get() + 1));
        assert_eq!(deferrals.get(), 1);
        assert_eq!(redirect_calls.get(), 1);
        assert_eq!(core_calls.get(), 1);

        assert!(redirect_save_hooks_install_ready(true, false));
        assert!(redirect_save_hooks_install_ready(false, true));
        assert!(!redirect_save_hooks_install_ready(false, false));
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
    fn classifies_copyfile_endpoints_for_wait_and_redirect() {
        let backup =
            wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2.bak");
        let plan = classify_copyfile_endpoint(&backup, |path| {
            let mut out = path.to_vec();
            out.push(0);
            Some(out)
        });
        assert!(plan.should_wait_for_missing_save_dialog);
        assert_eq!(plan.redirected.as_ref().and_then(|v| v.last()), Some(&0));

        let non_save = wide_path(r"C:\tmp\notes.txt");
        let plan = classify_copyfile_endpoint(&non_save, |_| None);
        assert!(!plan.should_wait_for_missing_save_dialog);
        assert!(plan.redirected.is_none());
    }

    #[test]
    fn plans_query_path_redirect_and_diagnostics() {
        let save = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2");
        let plan = plan_save_query_path(&save, |path| Some(path.to_vec()));
        assert!(plan.diag.contains_eldenring);
        assert!(plan.diag.contains_er0000);
        assert!(plan.diag.should_record_path_kind());
        assert_eq!(plan.redirected.as_deref(), Some(save.as_slice()));

        let other = wide_path(r"C:\tmp\other.bin");
        let plan = plan_save_query_path(&other, |_| None);
        assert!(!plan.diag.should_record_path_kind());
        assert!(plan.redirected.is_none());
    }

    #[test]
    fn classifies_query_paths_for_existence_diagnostics() {
        let save = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2");
        let diag = classify_save_query_path(&save);
        assert_eq!(
            diag,
            SaveQueryPathDiag {
                contains_eldenring: true,
                contains_er0000: true,
            }
        );
        assert!(diag.should_record_path_kind());
        assert!(diag.should_capture_save_file_query_log(0, 1));
        assert!(!diag.should_capture_save_file_query_log(1, 1));
        assert!(diag.should_capture_general_query_log(0, 1));

        let root = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing");
        let root_diag = classify_save_query_path(&root);
        assert!(root_diag.should_record_path_kind());
        assert!(!root_diag.should_capture_save_file_query_log(0, 8));

        assert!(
            !classify_save_query_path(&wide_path(r"C:\tmp\notes.txt")).should_record_path_kind()
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
    fn plans_createfile_open_redirect_and_side_effect_flags() {
        let save = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2");
        let plan = plan_create_file_open(&save, |path| Some(path.to_vec()));
        assert!(plan.diag.save_like);
        assert!(plan.should_wait_for_missing_save_dialog());
        assert!(plan.should_normalize_on_save_open());
        assert_eq!(plan.redirected.as_deref(), Some(save.as_slice()));

        let graphics = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\GraphicsConfig.xml");
        let plan = plan_create_file_open(&graphics, |_| None);
        assert!(plan.diag.save_like);
        assert!(!plan.should_wait_for_missing_save_dialog());
        assert!(!plan.should_normalize_on_save_open());
        assert!(plan.redirected.is_none());
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
    fn plans_save_path_telemetry_kind_and_bucket() {
        let staged = wide_path(
            r"Z:\tmp\er-effects-save-redirect-stage\eldenring\76561197960265729\ER0000.sl2",
        );
        let plan = plan_save_path_telemetry(&staged);
        assert_eq!(plan.kind, SavePathKind::StageSaveFile);
        assert_eq!(plan.bucket, Some(SavePathTelemetryBucket::StageSaveFile));

        let graphics = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\GraphicsConfig.xml");
        let plan = plan_save_path_telemetry(&graphics);
        assert_eq!(plan.kind, SavePathKind::GraphicsConfig);
        assert_eq!(plan.bucket, None);
    }

    #[test]
    fn exposes_telemetry_buckets_for_counted_save_path_kinds() {
        assert_eq!(
            SavePathKind::StageSteamIdDir.telemetry_bucket(),
            Some(SavePathTelemetryBucket::StageSteamIdDir)
        );
        assert_eq!(
            SavePathKind::StageSaveFile.telemetry_bucket(),
            Some(SavePathTelemetryBucket::StageSaveFile)
        );
        assert_eq!(
            SavePathKind::ConfiguredSaveFile.telemetry_bucket(),
            Some(SavePathTelemetryBucket::ConfiguredSaveFile)
        );
        assert_eq!(SavePathKind::GraphicsConfig.telemetry_bucket(), None);
        assert_eq!(SavePathKind::OtherSaveLike.telemetry_bucket(), None);
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
    fn builds_default_save_file_path_from_root_steamid_and_leaf() {
        let root = Path::new(r"C:\Users\x\AppData\Roaming\EldenRing");
        assert_eq!(
            default_save_file_path(root, 76561197960265729, "ER0000.sl2"),
            root.join("76561197960265729").join("ER0000.sl2")
        );
    }

    #[test]
    fn parses_plausible_steam_id64_dir_names() {
        assert_eq!(
            steam_id64_from_dir_name("76561197960265729"),
            Some(76561197960265729)
        );
        assert_eq!(steam_id64_from_dir_name("76561197960265729.bak"), None);
        assert_eq!(steam_id64_from_dir_name("not-a-steamid"), None);
        assert_eq!(steam_id64_from_dir_name("9999999999999999"), None);
    }

    #[test]
    fn validates_plausible_steam_id64_range() {
        assert_eq!(
            plausible_steam_id64(10_000_000_000_000_000),
            Some(10_000_000_000_000_000)
        );
        assert_eq!(
            plausible_steam_id64(99_999_999_999_999_999),
            Some(99_999_999_999_999_999)
        );
        assert_eq!(plausible_steam_id64(9_999_999_999_999_999), None);
        assert_eq!(plausible_steam_id64(100_000_000_000_000_000), None);
    }

    #[test]
    fn plans_direct_stage_request_from_steamid_or_no_steamid_kind() {
        let save = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729\ER0000.sl2");
        assert_eq!(
            plan_direct_stage_request(&save),
            DirectStageRequestPlan::SteamId(76561197960265729)
        );

        let graphics = wide_path(r"C:\Users\x\AppData\Roaming\EldenRing\GraphicsConfig.xml");
        assert_eq!(
            plan_direct_stage_request(&graphics),
            DirectStageRequestPlan::NoSteamId(DirectStageNoSteamIdKind::GraphicsConfig)
        );
    }

    #[test]
    fn builds_direct_stage_case_dirs() {
        let root = Path::new(r"C:\stage");
        let [lower, canonical] = direct_stage_case_dirs(root);
        assert_eq!(lower, root.join("eldenring"));
        assert_eq!(canonical, root.join("EldenRing"));
    }

    #[test]
    fn probes_direct_stage_file_status_for_the_active_modes_load_names() {
        let unique = format!(
            "er-save-redirect-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let steam_id = 76561197960265729_u64;
        let sl2 = root
            .join("EldenRing")
            .join(steam_id.to_string())
            .join("ER0000.sl2");
        std::fs::create_dir_all(sl2.parent().unwrap()).unwrap();
        std::fs::write(&sl2, b"sl2").unwrap();

        let vanilla_names = active_save_container_names_for(false, "ER0000.co2");
        let vanilla: Vec<&str> = vanilla_names.iter().map(String::as_str).collect();
        let seamless_names = active_save_container_names_for(true, "ER0000.co2");
        let seamless: Vec<&str> = seamless_names.iter().map(String::as_str).collect();
        // Vanilla loads only `.sl2`, and that is what is staged.
        assert_eq!(
            probe_direct_stage_file_status(Some(&root), &vanilla, steam_id),
            DirectStageFileStatus {
                exists: true,
                bytes: Some(3)
            }
        );
        // Seamless prefers `.co2` but accepts the `.sl2` that is present.
        assert_eq!(
            probe_direct_stage_file_status(Some(&root), &seamless, steam_id),
            DirectStageFileStatus {
                exists: true,
                bytes: Some(3)
            }
        );
        // A `.co2` present makes Seamless report the co-op container it will actually open, while
        // vanilla keeps reporting the `.sl2` it is locked to.
        let co2 = root
            .join("eldenring")
            .join(steam_id.to_string())
            .join("ER0000.co2");
        std::fs::create_dir_all(co2.parent().unwrap()).unwrap();
        std::fs::write(&co2, b"co2!!").unwrap();
        assert_eq!(
            probe_direct_stage_file_status(Some(&root), &seamless, steam_id),
            DirectStageFileStatus {
                exists: true,
                bytes: Some(5)
            }
        );
        assert_eq!(
            probe_direct_stage_file_status(Some(&root), &vanilla, steam_id),
            DirectStageFileStatus {
                exists: true,
                bytes: Some(3)
            }
        );
        assert_eq!(
            probe_direct_stage_file_status(Some(&root), &seamless, 0),
            DirectStageFileStatus {
                exists: false,
                bytes: None
            }
        );

        let _ = std::fs::remove_dir_all(root);
    }

    /// The Seamless container comes from ERSC's OWN config, not from a hard-coded `.co2`.
    #[test]
    fn reads_the_seamless_container_extension_out_of_ersc_settings() {
        // Verbatim shape of the shipped `ersc_settings.ini`, comment and all.
        let shipped = "[PASSWORD]\n\ncooppassword =seamless\n\n[SAVE]\n\n;Your save file extension (in the vanilla game this is .sl2). Use any alphanumeric characters (limit = 120)\nsave_file_extension = co2\n\n[LANGUAGE]\n\nmod_language_override =\n";
        assert_eq!(parse_ersc_save_file_extension(shipped), Some("co2"));
        assert_eq!(
            save_container_name_for_extension(parse_ersc_save_file_extension(shipped).unwrap()),
            "ER0000.co2"
        );

        // A user-chosen extension must flow through -- the case a hard-coded `.co2` breaks.
        assert_eq!(
            parse_ersc_save_file_extension("[SAVE]\nsave_file_extension = coop2\n"),
            Some("coop2")
        );
        // The key only counts inside `[SAVE]`.
        assert_eq!(
            parse_ersc_save_file_extension("[GAMEPLAY]\nsave_file_extension = nope\n"),
            None
        );
        // Absent, blank, commented out, or over-long -> no usable value.
        assert_eq!(parse_ersc_save_file_extension("[SAVE]\n"), None);
        assert_eq!(
            parse_ersc_save_file_extension("[SAVE]\nsave_file_extension =\n"),
            None
        );
        assert_eq!(
            parse_ersc_save_file_extension("[SAVE]\n;save_file_extension = co2\n"),
            None
        );
        assert_eq!(
            parse_ersc_save_file_extension(&format!(
                "[SAVE]\nsave_file_extension = {}\n",
                "a".repeat(MAX_SAVE_FILE_EXTENSION_LEN + 1)
            )),
            None
        );
        // A filename is built from this, so anything that could leave the directory is refused.
        for hostile in ["../../evil", "co2/x", r"co2\x", "co 2", "co.2"] {
            assert_eq!(
                parse_ersc_save_file_extension(&format!(
                    "[SAVE]\nsave_file_extension = {hostile}\n"
                )),
                None,
                "a non-alphanumeric extension must not reach a staged filename: {hostile}"
            );
        }
    }

    /// THE naming rule. Whatever container the runtime resolves to once the Seamless mode has
    /// settled, staging must already have written the configured source under that name -- for the
    /// DEFAULT co-op extension and for a custom one, and never varying with the SOURCE file's
    /// extension (the shape the 2026-08-11 soft lock was misdiagnosed as).
    #[test]
    fn staged_container_names_cover_every_mode_for_any_configured_extension() {
        for extension in [DEFAULT_SEAMLESS_SAVE_FILE_EXTENSION, "coop2", "sl2"] {
            let seamless_name = save_container_name_for_extension(extension);
            let staged = staged_save_container_names_for(&seamless_name);
            let staged_refs: Vec<&str> = staged.iter().map(String::as_str).collect();
            for seamless in [false, true] {
                assert!(
                    is_staged_save_container_name(
                        &active_save_container_name_for(seamless, &seamless_name),
                        &staged_refs
                    ),
                    "staging must write the container the runtime writes (ext={extension} seamless={seamless})"
                );
                for name in active_save_container_names_for(seamless, &seamless_name) {
                    assert!(
                        is_staged_save_container_name(&name, &staged_refs),
                        "staging must write every container the runtime may load: {name} (ext={extension})"
                    );
                }
            }
            // Vanilla is locked to `.sl2` whatever ERSC is configured with.
            assert_eq!(
                active_save_container_names_for(false, &seamless_name),
                vec!["ER0000.sl2"]
            );
        }

        // `.co2` is a default, not an invariant: a custom extension names a different container.
        assert_eq!(
            active_save_container_names_for(true, "ER0000.coop2"),
            vec!["ER0000.coop2", "ER0000.sl2"]
        );
        assert_eq!(
            staged_save_container_names_for("ER0000.coop2"),
            vec!["ER0000.sl2", "ER0000.coop2"]
        );
        // An ERSC configured with `sl2` IS the vanilla container -- one name, never duplicated.
        assert_eq!(
            staged_save_container_names_for("ER0000.sl2"),
            vec!["ER0000.sl2"]
        );
        assert_eq!(
            active_save_container_names_for(true, "ER0000.sl2"),
            vec!["ER0000.sl2"]
        );

        let default_staged = staged_save_container_names_for("ER0000.co2");
        let default_refs: Vec<&str> = default_staged.iter().map(String::as_str).collect();
        assert!(is_staged_save_container_name("er0000.CO2", &default_refs));
        assert!(!is_staged_save_container_name(
            "ER0000.sl2.bak",
            &default_refs
        ));
        assert!(!is_staged_save_container_name("ER0001.sl2", &default_refs));
        // The staged set never depends on the SOURCE file's extension: it is the same set whether
        // the configured save was picked as a `.sl2`, a `.co2`, or anything else.
        assert_eq!(default_staged, vec!["ER0000.sl2", "ER0000.co2"]);
    }

    /// THE staleness check. A container from an earlier run that this pass does not rewrite is
    /// removed, so it can never be served in place of the configured source.
    #[test]
    fn staged_entry_fate_removes_leftovers_and_keeps_non_save_files() {
        let staged = ["ER0000.sl2", "ER0000.co2"];
        assert_eq!(
            staged_entry_fate("ER0000.sl2", &staged),
            StagedEntryFate::Rewritten
        );
        assert_eq!(
            staged_entry_fate("er0000.co2", &staged),
            StagedEntryFate::Rewritten
        );

        // The 2026-08-11 leftovers: a `.bak` companion and a restore temp from earlier sessions.
        assert_eq!(
            staged_entry_fate("ER0000.co2.bak", &staged),
            StagedEntryFate::StaleRemove
        );
        assert_eq!(
            staged_entry_fate("ER0000.sl2.bak", &staged),
            StagedEntryFate::StaleRemove
        );
        assert_eq!(
            staged_entry_fate("er0000.sl2.er-save-dest-restore.tmp", &staged),
            StagedEntryFate::StaleRemove
        );
        assert_eq!(
            staged_entry_fate("ER0001.sl2", &staged),
            StagedEntryFate::StaleRemove
        );

        // A container left by a PREVIOUS ERSC extension is exactly what must not survive...
        assert_eq!(
            staged_entry_fate("ER0000.coop2", &staged),
            StagedEntryFate::StaleRemove
        );
        // ...and it is kept once ERSC is configured that way.
        assert_eq!(
            staged_entry_fate("ER0000.coop2", &["ER0000.sl2", "ER0000.coop2"]),
            StagedEntryFate::Rewritten
        );

        assert_eq!(
            staged_entry_fate("GraphicsConfig.xml", &staged),
            StagedEntryFate::Keep
        );
        assert_eq!(
            staged_entry_fate("er-effects-autoload-debug.log", &staged),
            StagedEntryFate::Keep
        );
    }

    #[test]
    fn stage_deletes_are_confined_to_the_private_stage_tree() {
        assert!(is_inside_direct_stage_root(Path::new(
            "/home/u/save-files/125-Frenzy/er-effects-save-redirect-stage/eldenring/765/"
        )));
        assert!(!is_inside_direct_stage_root(Path::new(
            "/home/u/save-files/125-Frenzy"
        )));
        assert!(!is_inside_direct_stage_root(Path::new(
            r"C:\Users\x\AppData\Roaming\EldenRing\76561197960265729"
        )));
    }

    #[test]
    fn dedupes_case_dirs_that_resolve_to_one_directory() {
        let root = Path::new("/stage");
        let [lower, native] = direct_stage_case_dirs(root);
        // Wine/case-insensitive host: both spellings resolve to the same directory, so the 28 MB
        // container is written once.
        let merged = dedupe_dirs_by_identity([lower.clone(), native.clone()], |dir| {
            Some(PathBuf::from(dir.to_string_lossy().to_lowercase()))
        });
        assert_eq!(merged, vec![lower.clone()]);

        // Case-sensitive host: two real directories, both written.
        let split = dedupe_dirs_by_identity([lower.clone(), native.clone()], |dir| {
            Some(dir.to_path_buf())
        });
        assert_eq!(split, vec![lower.clone(), native.clone()]);

        // Unresolvable paths fall back to their own literal path and are never merged.
        let unresolved = dedupe_dirs_by_identity([lower.clone(), native.clone()], |_| None);
        assert_eq!(unresolved, vec![lower, native]);
    }

    #[test]
    fn reports_save_file_readonly_status() {
        let unique = format!(
            "er-save-redirect-readonly-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let file = std::env::temp_dir().join(unique);
        std::fs::write(&file, b"save").unwrap();
        assert!(!save_file_is_readonly(&file));
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&file, perms).unwrap();
        assert!(save_file_is_readonly(&file));
        let mut perms = std::fs::metadata(&file).unwrap().permissions();
        // Restoring write permission on the throwaway temp file this test created, so the
        // `remove_file` below succeeds. Deliberately NOT narrowed to owner-write: the value under
        // test is `save_file_is_readonly`, and the repo rule that the live game-owned save must stay
        // writable makes "clear the readonly bit" the exact semantics to exercise here.
        #[allow(
            clippy::permissions_set_readonly_false,
            reason = "test-local temp file: clearing the readonly bit is the behaviour under test and the file is deleted on the next line"
        )]
        perms.set_readonly(false);
        std::fs::set_permissions(&file, perms).unwrap();
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checks_writeback_allowed_under_default_root_case_insensitively() {
        let default_root = Path::new(r"C:\Users\x\AppData\Roaming\EldenRing");
        let save = Path::new(r"c:/users/x/appdata/roaming/eldenring/76561197960265729/ER0000.sl2");
        assert!(save_file_writeback_allowed(save, Some(default_root)));

        let staged = Path::new(
            r"Z:\tmp\er-effects-save-redirect-stage\eldenring\76561197960265729\ER0000.sl2",
        );
        assert!(!save_file_writeback_allowed(staged, Some(default_root)));
        assert!(!save_file_writeback_allowed(save, None));
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
