
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
    if (csidl & CSIDL_FOLDER_MASK) == CSIDL_APPDATA && !path.is_null() {
        SAVE_REDIRECT_SHGFP_APPDATA_REQUESTS.fetch_add(1, Ordering::SeqCst);
        if SAVE_FIRST_LOAD_DONE.load(Ordering::SeqCst) {
            SAVE_REDIRECT_SHGFP_FIRST_LOAD_DONE_BLOCKS.fetch_add(1, Ordering::SeqCst);
        } else if let Some(root) = SAVE_REDIRECT_DIR_W.get() {
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
                if is_save_file_or_backup_path(path) {
                    wait_for_missing_save_dialog_if_pending(path);
                }
                if is_sl2 {
                    observe_steam_id64_from_save_path(path);
                    let is_write = access & 0x4000_0000 != 0 || access & 0x2 != 0;
                    if !is_write {
                        if let Ok(base) = game_module_base() {
                            normalize_env_save_file_to_active_steam_id_once(
                                base,
                                "ntcreatefile-save-open",
                            );
                        }
                    }
                }
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
        // Rate-limit: log the first 8 .sl2 opens, then only at power-of-two hit counts (the capture
        // pre-gate above still bounds this counter at SAVE_NTCREATE_DIAG_MAX).
        let hits = SAVE_NTCREATE_DIAG_LOGGED.fetch_add(1, Ordering::SeqCst) + 1;
        if hits <= 8 || hits.is_power_of_two() {
            // ret is NTSTATUS (0 == STATUS_SUCCESS). is_write keys off GENERIC_WRITE (0x40000000)
            // or FILE_WRITE_DATA (0x2) so a failing save COMMIT is unambiguous in the log.
            let is_write = access & 0x4000_0000 != 0 || access & 0x2 != 0;
            append_autoload_debug(format_args!(
                "save-override: NtCreateFile diag access=0x{access:x} disp={disposition} opts=0x{options:x} write={is_write} sl2={is_sl2} diag_hits={hits} '{p}'"
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
    let missing_save_pending = MISSING_SAVE_DIALOG_STATE.load(Ordering::SeqCst) == MISSING_SAVE_DIALOG_PENDING;
    if SAVE_REDIRECT_DIR_W.get().is_none() && !save_trace_enabled() && !missing_save_pending {
        append_autoload_debug(format_args!(
            "save-override: install deferred -- redirect dir not set yet (waiting for missing-save picker/configured source)"
        ));
        return;
    }
    SAVE_REDIRECT_INSTALL_ONCE.call_once(|| {
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
