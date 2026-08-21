use super::*;

use er_save_redirect::{
    SAVE_REDIRECT_ORIG_NTCREATEFILE, SAVE_REDIRECT_ORIG_NTQUERYVOLINFO,
    SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW, SHGFP_MAX_PATH_W, SaveNtCreateDetourGuard,
    SaveRedirectHookDetours, classify_nt_create_file_save_path,
    fill_get_disk_free_space_ex_outputs, install_core_createfilew_hook,
    install_redirect_save_hooks_when_ready, is_ntquery_volume_free_space_class,
    nt_createfile_diag_hit_should_log, ntquery_volume_available_units,
    patch_ntquery_volume_free_space, shgetfolderpath_is_appdata_request,
    shgetfolderpath_staged_appdata_len, write_shgetfolderpath_staged_root,
};

type ShGetFolderPathWFn = unsafe extern "system" fn(isize, i32, isize, u32, *mut u16) -> i32;

/// SHGetFolderPathW detour: for CSIDL_APPDATA, return our staged ROOT instead of the real %APPDATA%,
/// so the game's save-dir builder produces `<our_root>\EldenRing\<steamid>\...` and reads our gold
/// save's character natively. All other folders pass through unchanged.
pub(super) unsafe extern "system" fn save_redirect_shgetfolderpathw_hook(
    hwnd: isize,
    csidl: i32,
    token: isize,
    flags: u32,
    path: *mut u16,
) -> i32 {
    const S_OK: i32 = 0;
    // One-shot: after the first gold load, revert to the real %APPDATA% so writes + subsequent loads
    // use the proper default C: dir (the Z: redirect only serves the first read of the gold).
    if shgetfolderpath_is_appdata_request(csidl) && !path.is_null() {
        SAVE_REDIRECT_SHGFP_APPDATA_REQUESTS.fetch_add(1, Ordering::SeqCst);
        let first_load_done = SAVE_FIRST_LOAD_DONE.load(Ordering::SeqCst);
        if first_load_done {
            SAVE_REDIRECT_SHGFP_FIRST_LOAD_DONE_BLOCKS.fetch_add(1, Ordering::SeqCst);
        } else if let Some(root) = SAVE_REDIRECT_DIR_W.get() {
            if let Some(n) = shgetfolderpath_staged_appdata_len(
                csidl,
                first_load_done,
                Some(root.len()),
                SHGFP_MAX_PATH_W,
            ) {
                let n = unsafe { write_shgetfolderpath_staged_root(path, root, n) };
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
        } else {
            SAVE_REDIRECT_SHGFP_NO_ROOT_BLOCKS.fetch_add(1, Ordering::SeqCst);
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
pub(super) unsafe extern "system" fn save_ntcreatefile_diag_hook(
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
    // RE-ENTRANCY (see `reentry.rs`): this is the LOWEST layer of the family -- kernel32's own
    // `CreateFileW` calls it, so it fires again under every Win32 open a detour above already
    // handled, and again under every `fs::read`/`fs::write` those detours perform. A nested entry
    // here is by definition the ntdll leg of an open that was already observed and logged upstairs,
    // so it carries no new information: pass straight through. This token adds no DEPTH on purpose
    // -- ntdll sits beneath the Win32 detours rather than beside them, and counting it would put a
    // perfectly healthy open at depth 2 and make the max-depth alarm useless.
    let ntdll_detour = SaveNtCreateDetourGuard::enter();
    if ntdll_detour.is_reentrant() {
        let orig = SAVE_REDIRECT_ORIG_NTCREATEFILE.load(Ordering::SeqCst);
        let call: NtCreateFileFn = unsafe { std::mem::transmute::<usize, NtCreateFileFn>(orig) };
        return unsafe {
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
    }
    let mut save_diag: Option<(String, bool, bool)> = None;
    if !object_attributes.is_null() {
        let objname = unsafe { *(object_attributes.add(0x10) as *const usize) } as *const u8;
        if !objname.is_null() {
            let len_bytes = unsafe { *(objname as *const u16) } as usize;
            let buf = unsafe { *(objname.add(0x08) as *const usize) } as *const u16;
            if !buf.is_null() && (2..0x2000).contains(&len_bytes) {
                let nwch = len_bytes / 2;
                let path = unsafe { std::slice::from_raw_parts(buf, nwch) };
                // Focus the (capped) budget on ER0000.sl2 opens ONLY -- early boot churns hundreds
                // of "eldenring"-dir opens (graphicsconfig.xml, etc.) that otherwise exhaust the cap
                // before the boot save READ/WRITE we care about. The .sl2 opens ARE the save commit.
                let diag = classify_nt_create_file_save_path(path, access);
                if diag.should_wait_for_missing_save_dialog() {
                    wait_for_missing_save_dialog_if_pending(path);
                }
                if diag.should_observe_steam_id() {
                    observe_steam_id64_from_save_path(path);
                    if diag.should_normalize_on_read()
                        && let Ok(base) = game_module_base()
                    {
                        normalize_env_save_file_to_active_steam_id_once(
                            base,
                            "ntcreatefile-save-open",
                        );
                    }
                }
                if diag.should_capture_diag_log(
                    SAVE_NTCREATE_DIAG_LOGGED.load(Ordering::SeqCst),
                    SAVE_NTCREATE_DIAG_MAX,
                ) {
                    // UTF-8 Lossy: log-only decode of an NT path for probe diagnosis.
                    save_diag = Some((String::from_utf16_lossy(path), diag.is_sl2, diag.is_write));
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
    if let Some((p, is_sl2, is_write)) = save_diag {
        // Rate-limit: log the first 8 .sl2 opens, then only at power-of-two hit counts (the capture
        // pre-gate above still bounds this counter at SAVE_NTCREATE_DIAG_MAX).
        let hits = SAVE_NTCREATE_DIAG_LOGGED.fetch_add(1, Ordering::SeqCst) + 1;
        if nt_createfile_diag_hit_should_log(hits) {
            // ret is NTSTATUS (0 == STATUS_SUCCESS). is_write keys off GENERIC_WRITE (0x40000000)
            // or FILE_WRITE_DATA (0x2) so a failing save COMMIT is unambiguous in the log.
            append_autoload_debug(format_args!(
                "save-override: NtCreateFile diag access=0x{access:x} disp={disposition} opts=0x{options:x} write={is_write} sl2={is_sl2} diag_hits={hits} '{p}'"
            ));
        }
    }
    ret
}

/// GetDiskFreeSpaceExW detour: for the EldenRing save dir, report ample free space (Wine returns
/// bogus 0 on the Z:->/home drive, which fails the save-commit free-space precheck -> corrupted-save
/// loop). Everything else passes through unchanged.
pub(super) unsafe extern "system" fn save_redirect_getdiskfreew_hook(
    lp_dir: *const u16,
    free_avail: *mut u64,
    total: *mut u64,
    total_free: *mut u64,
) -> i32 {
    // Override EVERY call (the game's save-commit precheck may pass the bare drive root, not an
    // EldenRing path -- diag showed it never matched the eldenring filter). Returning ample free is
    // benign for a probe and guarantees the `free < needed` precheck passes. Log the first few paths.
    unsafe { fill_get_disk_free_space_ex_outputs(free_avail, total, total_free) };
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
pub(super) unsafe extern "system" fn save_redirect_ntqueryvolinfo_hook(
    handle: isize,
    iosb: *mut u8,
    fs_info: *mut u8,
    length: u32,
    fs_class: u32,
) -> i32 {
    let orig = SAVE_REDIRECT_ORIG_NTQUERYVOLINFO.load(Ordering::SeqCst);
    let call: NtQueryVolumeInfoFn =
        unsafe { std::mem::transmute::<usize, NtQueryVolumeInfoFn>(orig) };
    let ret = unsafe { call(handle, iosb, fs_info, length, fs_class) };
    // DIAGNOSTIC: log only the FREE-SPACE classes (3/7), capped. Logging every class exhausts the cap
    // on early-boot class=1 spam before the save-time free-space precheck fires; the precheck is the
    // only thing that matters for the corrupted-save loop. pre_avail_units = the bogus Wine value.
    if is_ntquery_volume_free_space_class(fs_class) {
        let d = SAVE_VOLINFO_LOGGED.load(Ordering::SeqCst);
        if d < 40 {
            SAVE_VOLINFO_LOGGED.store(d + 1, Ordering::SeqCst);
            let avail = unsafe { ntquery_volume_available_units(ret, fs_info, length, fs_class) }
                .unwrap_or(-1);
            append_autoload_debug(format_args!(
                "save-override: NtQueryVolumeInformationFile diag class={fs_class} len={length} ret=0x{ret:x} pre_avail_units={avail}"
            ));
        }
    }
    if unsafe { patch_ntquery_volume_free_space(ret, fs_info, length, fs_class) } {
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
pub(super) unsafe fn module_proc(module_name: &[u8], proc_name: &[u8]) -> usize {
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

/// Install the CreateFileW detour ALONE, unconditionally, in every save mode (save-game-flow WP3).
///
/// The detour body is pass-through-safe on its own: every redirect decision it makes needs either
/// `SAVE_REDIRECT_DIR_W` (unset in default game-owned-APPDATA mode) or the save-flow's armed
/// destination window, so without those it only observes. The save-destination commit rides THIS
/// hook, which is why it can no longer be gated on the redirect mode. Everything else in
/// `install_save_redirect_hooks` -- and especially the Wine-only free-space overrides -- stays
/// behind its own gate. Idempotent; safe to call from both installers.
pub(crate) fn install_save_file_core_hooks() {
    unsafe {
        install_core_createfilew_hook(
            &SAVE_HOOK_INSTALL_STATE,
            save_redirect_createfilew_hook as *mut c_void,
            |name| kernel32_proc(name),
            |message| append_autoload_debug(format_args!("{message}")),
        );
    }
}

/// True once the core `kernel32!CreateFileW` detour is live.
///
/// The OS file dialog gates on this. A modal common dialog performs a great deal of shell file I/O
/// on the calling thread, which re-enters that detour; installing a MinHook while a thread is parked
/// inside comdlg32 is the one shape that could deadlock, because `MH_ApplyQueued` suspends every
/// other thread and allocates while they are frozen. Every installer in this DLL is attach-time and
/// long finished before a user can reach System>Quit, so refusing to open a dialog until the detour
/// has settled removes the overlap entirely rather than reasoning about it.
pub(crate) fn save_file_core_hooks_live() -> bool {
    SAVE_HOOK_INSTALL_STATE.core_createfilew_installed()
}

pub(crate) fn install_save_redirect_hooks() {
    // While the in-game missing-save picker is pending the hooks stay UNINSTALLED on purpose:
    // native save IO must flow so the title completes its no-save boot and the 05_010 file
    // browser can present itself. `complete_missing_save_selection_from_picker` re-invokes this
    // installer right after activating the picked source (the install body is Once-guarded).
    unsafe {
        install_redirect_save_hooks_when_ready(
            &SAVE_HOOK_INSTALL_STATE,
            SAVE_REDIRECT_DIR_W.get().is_some(),
            save_trace_enabled(),
            SaveRedirectHookDetours {
                copyfilew: save_redirect_copyfilew_hook as *mut c_void,
                get_file_attributes_w: save_redirect_getattrw_hook as *mut c_void,
                get_file_attributes_ex_w: save_redirect_getattrexw_hook as *mut c_void,
                find_first_file_w: save_redirect_findfirstw_hook as *mut c_void,
                get_disk_free_space_ex_w: save_redirect_getdiskfreew_hook as *mut c_void,
                sh_get_folder_path_w: save_redirect_shgetfolderpathw_hook as *mut c_void,
                nt_query_volume_information_file: save_redirect_ntqueryvolinfo_hook as *mut c_void,
                nt_create_file: save_ntcreatefile_diag_hook as *mut c_void,
            },
            running_under_wine(),
            |name| kernel32_proc(name),
            |name| module_proc(b"shell32.dll\0", name),
            |name| module_proc(b"ntdll.dll\0", name),
            install_save_file_core_hooks,
            |message| append_autoload_debug(format_args!("{message}")),
        );
    }
}
