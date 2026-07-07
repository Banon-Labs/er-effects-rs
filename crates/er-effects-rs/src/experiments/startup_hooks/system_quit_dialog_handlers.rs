
unsafe fn system_quit_open_profile_load_dialog(action_obj: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- module base unavailable"
        ));
        return false;
    };
    let system_dialog = unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(NULL);
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- action=0x{action_obj:x} dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    let scene_proxy = system_dialog + SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET;
    let scene_proxy_vt = unsafe { safe_read_usize(scene_proxy) }.unwrap_or(NULL);
    let want_scene_proxy_vt = base + SCENE_OBJ_PROXY_VTABLE_RVA;
    if scene_proxy_vt != want_scene_proxy_vt {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- dialog=0x{system_dialog:x} scene_proxy=dialog+0x{SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET:x}=0x{scene_proxy:x} vt=0x{scene_proxy_vt:x} want=0x{want_scene_proxy_vt:x}"
        ));
        return false;
    }
    // Native title/menu route callers pass `owner + 0x50` as the MenuWindowJob's
    // field2_0x50 list argument. MenuWindowJob::Run later appends the loaded
    // owning MenuWindow to this DLFixedVector via FUN_140733ff0. Passing the
    // SceneObjProxy backref here is wrong: it lets the resource load start, then
    // asserts in DLFixedVector.inl line 0x296 when Run appends to a full/wrong
    // object.
    let menu_window_list = system_dialog + 0x50;
    let menu_window_list_count = unsafe { safe_read_usize(menu_window_list + 0x48) }.unwrap_or(!0);
    if menu_window_list_count >= 8 {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- candidate menu_window_list=dialog+0x50=0x{menu_window_list:x} count@+0x48={menu_window_list_count} would overflow DLFixedVector<8>"
        ));
        return false;
    }
    let Ok(wrapper_addr) = game_rva(PROFILE_SELECT_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve ProfileSelect wrapper rva 0x{PROFILE_SELECT_WRAPPER_RVA:x}"
        ));
        return false;
    };
    let Ok(submit_addr) = game_rva(MENU_JOB_SUBMIT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve menu-job submit rva 0x{MENU_JOB_SUBMIT_RVA:x}"
        ));
        return false;
    };
    let job_slot = &SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT as *const AtomicUsize as usize;
    SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.store(NULL, Ordering::SeqCst);
    let wrapper: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(wrapper_addr) };
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route FIRE 05_010_ProfileSelect wrapper 0x{wrapper_addr:x}(rcx=job_slot=0x{job_slot:x}, rdx=menu_window_list=dialog+0x50=0x{menu_window_list:x} count={menu_window_list_count}, r8=scene_proxy=0x{scene_proxy:x}) from system_dialog=0x{system_dialog:x}"
    ));
    let ret = unsafe { wrapper(job_slot, menu_window_list, scene_proxy) };
    let job = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    let job_vt = if job >= HEAP_LO {
        unsafe { safe_read_usize(job) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if job < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route 05_010 wrapper returned=0x{ret:x} job_slot=0x{job_slot:x} job=0x{job:x} job_vt=0x{job_vt:x}; no job to submit"
        ));
        return false;
    }
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(submit_addr) };
    let submit_queue = system_dialog + 0x10;
    SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.store(menu_window_list, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.store(system_dialog, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(system_dialog, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route SUBMIT job=0x{job:x} job_vt=0x{job_vt:x} via 0x{submit_addr:x}(queue=dialog+0x10=0x{submit_queue:x}, job_slot=0x{job_slot:x}); armed ProfileSelect list observer=0x{menu_window_list:x} -- no slot activation/no load"
    ));
    unsafe { submit(submit_queue, job_slot) };
    let job_after_submit = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route submitted 05_010 wrapper job; job_slot_after=0x{job_after_submit:x}"
    ));
    true
}

pub(crate) unsafe extern "system" fn system_quit_menu_window_list_push_hook(
    list: usize,
    window: usize,
) -> u8 {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let orig = SYSTEM_QUIT_WINDOW_LIST_PUSH_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: MenuWindow list push trampoline unset for list=0x{list:x} window=0x{window:x} -- fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { original(list, window) };
    let armed_list = SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.load(Ordering::SeqCst);
    let system_dialog = SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.load(Ordering::SeqCst);
    if armed_list == 0 || armed_list != list || system_dialog == 0 {
        return ret;
    }
    SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.store(0, Ordering::SeqCst);
    let count = unsafe { safe_read_usize(list + 0x48) }.unwrap_or(0);
    let slot0 = unsafe { safe_read_usize(system_quit_list_slot_addr(list, 0)) }.unwrap_or(NULL);
    let slot1 = if count > 1 {
        unsafe { safe_read_usize(system_quit_list_slot_addr(list, 1)) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let top_window = slot0;
    let top_vt = if top_window >= HEAP_LO {
        unsafe { safe_read_usize(top_window) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let top_id = if top_window >= HEAP_LO {
        unsafe { safe_read_u16(top_window + 0x180) }.unwrap_or(u16::MAX)
    } else {
        u16::MAX
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect append observed list=0x{list:x} dialog=0x{system_dialog:x} count={count} slot0/top=0x{slot0:x} top_vt=0x{top_vt:x} top_id=0x{top_id:x} slot1=0x{slot1:x} appended_window=0x{window:x} ret={ret}"
    ));
    SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW.store(window, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_LIST.store(list, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID.store(top_id as usize, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn system_quit_noop_desktop_action_hook(
    action_obj: usize,
) -> usize {
    let open_save_dir_action = SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT.load(Ordering::SeqCst);
    if action_obj != 0 && action_obj == open_save_dir_action {
        SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = unsafe { system_quit_open_env_save_dir() };
        append_autoload_debug(format_args!(
            "system-quit-open-save-dir: cloned action selected action=0x{action_obj:x} opened={opened}; suppressing native Quit Game row action"
        ));
        return 0;
    }
    let recorded_action = SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.load(Ordering::SeqCst);
    if action_obj != 0 && action_obj == recorded_action {
        let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
        if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
            append_autoload_debug(format_args!(
                "system-quit-dup: cloned quick-load action re-entry ignored action=0x{action_obj:x} phase={phase}; native handoff already armed"
            ));
            return 0;
        }
        SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = unsafe { system_quit_open_profile_load_dialog(action_obj) };
        append_autoload_debug(format_args!(
            "system-quit-dup: cloned quick-load action selected action=0x{action_obj:x} opened={opened}; suppressing native Quit Game row action until ProfileSelect confirms slot"
        ));
        return 0;
    }
    let dialog = unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(0);
    if dialog >= 0x10000 {
        SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.store(0, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let closed = unsafe { system_quit_save_game_fire_save_and_close(dialog, "row_action") };
        append_autoload_debug(format_args!(
            "system-quit-save: Save Game row selected action=0x{action_obj:x} dialog=0x{dialog:x}; requested save + close-all closed={closed}; suppressed native Quit Game/return-title action"
        ));
        return 0;
    }
    let orig = SYSTEM_QUIT_NOOP_ACTION_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-save: Quit Game/Save Game action trampoline is unset for action=0x{action_obj:x} dialog=0x{dialog:x}; fail-open return 0"
        ));
        return 0;
    }
    append_autoload_debug(format_args!(
        "system-quit-save: original Quit Game row selected action=0x{action_obj:x} but dialog=0x{dialog:x} is not heap-like; forwarding native action without save-only latch"
    ));
    let original: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { original(action_obj) }
}

fn wide_z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// The ACTIVE save file the character-switch feature snapshots + restores + writes to. Resolved from
/// runtime GROUND TRUTH via `active_save_file_for_system_quit()`: a direct-file save selected in the
/// missing-save picker is a read-only source copied into the private redirected native save tree, so
/// this returns the game's native `%APPDATA%/EldenRing/<steamid>/ER0000.{co2|sl2}` path for writes.
/// Explicit/default saves keep using the normal configured/default resolver. Never write back to the
/// direct source file under `save-files/` or a user-picked path.
fn system_quit_env_save_path() -> Result<String, &'static str> {
    let Some(path) = active_save_file_for_system_quit() else {
        return Err("no active save file (direct/configured save unset and no default ER0000 save resolved)");
    };
    let path = path.to_string_lossy();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("resolved active save file is blank");
    }
    Ok(trimmed.trim_end_matches(['/', '\\']).to_owned())
}

fn system_quit_env_save_dir() -> Result<String, &'static str> {
    let trimmed = system_quit_env_save_path()?;
    let Some(sep) = trimmed.rfind(['/', '\\']) else {
        return Err("configured save_file has no parent directory");
    };
    let dir = &trimmed[..sep];
    if dir.is_empty() {
        return Err("configured save_file parent directory is empty");
    }
    Ok(dir.to_owned())
}

fn system_quit_path_for_windows(path: &str) -> Vec<u16> {
    let mut win = if path.starts_with('/') {
        format!("Z:{}", path.replace('/', "\\"))
    } else {
        path.replace('/', "\\")
    };
    while win.ends_with('\\') && win.len() > 3 {
        win.pop();
    }
    wide_z(&win)
}

fn system_quit_path_from_windows_picker(path: &[u16]) -> Option<String> {
    let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
    if end == 0 {
        return None;
    }
    String::from_utf16(&path[..end]).ok()
}

fn system_quit_windows_path_for_log(path: &str) -> String {
    if let Some(rest) = path
        .strip_prefix("Z:\\")
        .or_else(|| path.strip_prefix("z:\\"))
    {
        format!("/{}", rest.replace('\\', "/"))
    } else {
        path.to_owned()
    }
}

unsafe fn system_quit_open_env_save_dir() -> bool {
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-load-save-profiles: refused to open picker -- {reason}"
            ));
            return false;
        }
    };
    let dir = match system_quit_env_save_dir() {
        Ok(dir) => dir,
        Err(reason) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-load-save-profiles: refused to open picker -- {reason}"
            ));
            return false;
        }
    };
    if !Path::new(&dir).is_dir() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: refused to open picker for missing/non-directory save dir '{dir}'"
        ));
        return false;
    }
    unsafe { system_quit_save_swap_restore_profile_summary("load-save-profiles-reopen") };
    if !system_quit_save_swap_arm_original(&save_path) {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        return false;
    }

    let initial_dir_w = system_quit_path_for_windows(&dir);
    let title_w = wide_z("Load Save Profiles");
    // Mode-locked filter: Seamless Co-op (ERSC resident) reads/writes `ER0000.co2`, vanilla reads
    // `ER0000.sl2` -- offering the other flavor here would stage a save the active runtime never
    // loads (mixing save flavors across modes corrupts expectations; user directive 2026-07-06).
    // No "All files" escape hatch, for the same reason: the picker must offer ONLY the active
    // mode's container.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-load-save-profiles");
    let (filter_w, picker_ext) = if seamless {
        (wide_z("Seamless save (*.co2)\0*.co2\0"), "co2")
    } else {
        (wide_z("Elden Ring save (*.sl2)\0*.sl2\0"), "sl2")
    };
    let mut file_buf = [0u16; 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR::from_raw(filter_w.as_ptr()),
        lpstrFile: windows::core::PWSTR::from_raw(file_buf.as_mut_ptr()),
        nMaxFile: file_buf.len() as u32,
        lpstrInitialDir: PCWSTR::from_raw(initial_dir_w.as_ptr()),
        lpstrTitle: PCWSTR::from_raw(title_w.as_ptr()),
        Flags: OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY
            | OFN_NOCHANGEDIR
            | OFN_DONTADDTORECENT,
        ..Default::default()
    };
    let picked = unsafe { GetOpenFileNameW(&mut ofn).as_bool() };
    if !picked {
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: picker cancelled/no selection dir='{dir}'"
        ));
        return false;
    }
    let Some(selected_path) = system_quit_path_from_windows_picker(&file_buf) else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: picker returned an empty path"
        ));
        return false;
    };
    let selected_log = system_quit_windows_path_for_log(&selected_path);
    if !Path::new(&selected_path).is_file() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected path is not a file '{}'",
            selected_log
        ));
        return false;
    }
    // The filter above is display-only -- GetOpenFileNameW still returns whatever path the user
    // types -- so enforce the same mode lock on the picked file. A cross-flavor save (`.sl2` while
    // Seamless owns the session, `.co2` in vanilla) would preview character slots the active runtime
    // never actually loads (mixing save flavors across modes corrupts expectations; user directive
    // 2026-07-06).
    let ext_ok = Path::new(&selected_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(picker_ext));
    if !ext_ok {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: rejected '{}' -- picker is mode-locked to .{picker_ext} (seamless={seamless})",
            selected_log
        ));
        return false;
    }
    let Ok(mut bytes) = fs::read(&selected_path) else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: failed to read selected save '{}'",
            selected_log
        ));
        return false;
    };
    let len = bytes.len() as u64;
    let raw_hash = system_quit_hash_bytes(&bytes);
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save is not a valid BND4 '{}' len={len} hash=0x{raw_hash:016x}",
            selected_log
        ));
        return false;
    }
    let Ok(base) = game_module_base() else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save '{}' is valid but game module base is unavailable",
            selected_log
        ));
        return false;
    };
    normalize_save_bytes_to_active_steam_id(base, &mut bytes, "system-quit-picker-selection");
    let hash = system_quit_hash_bytes(&bytes);
    let mask = unsafe { system_quit_apply_foreign_profile_summary_preview(base, &bytes) };
    if mask == 0 {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save had no readable character slots '{}' len={len} hash=0x{hash:016x}",
            selected_log
        ));
        return false;
    }
    {
        let mut st = system_quit_save_swap_lock();
        st.candidate_bytes = bytes;
        st.candidate_hash = hash;
        st.candidate_slot_mask = mask;
        st.preview_applied = true;
    }
    SYSTEM_QUIT_OPEN_SAVE_DIR_SUCCESS_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-load-save-profiles: applied selected save preview '{}' len={len} hash=0x{hash:016x} slot_mask=0x{mask:x}; staged active save remains '{}' until a foreign slot is selected",
        selected_log, save_path
    ));
    true
}

unsafe fn system_quit_init_menu_string_from_static_wide(out: usize, text: &'static [u16]) -> bool {
    let Ok(ctor_addr) = game_rva(MENU_STRING_FROM_WIDE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve MenuString ctor rva 0x{MENU_STRING_FROM_WIDE_RVA:x}; cannot build static label"
        ));
        return false;
    };
    let ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(ctor_addr) };
    unsafe { ctor(out, text.as_ptr() as usize) };
    true
}

unsafe fn system_quit_build_static_label_component(
    out: usize,
    label: &'static [u16],
    help: &'static [u16],
) -> bool {
    unsafe { std::ptr::write_bytes(out as *mut u8, 0, MENU_HELP_LABEL_SIZE) };
    let label_ok = unsafe { system_quit_init_menu_string_from_static_wide(out, label) };
    let help_ok = unsafe {
        system_quit_init_menu_string_from_static_wide(out + MENU_HELP_LABEL_HELP_OFFSET, help)
    };
    label_ok && help_ok
}

const SYSTEM_QUIT_LOAD_PROFILE_LABEL_W: [u16; 13] = [
    b'L' as u16,
    b'o' as u16,
    b'a' as u16,
    b'd' as u16,
    b' ' as u16,
    b'P' as u16,
    b'r' as u16,
    b'o' as u16,
    b'f' as u16,
    b'i' as u16,
    b'l' as u16,
    b'e' as u16,
    0,
];

const SYSTEM_QUIT_LOAD_PROFILE_HELP_W: [u16; 37] = [
    b'S' as u16,
    b'e' as u16,
    b'l' as u16,
    b'e' as u16,
    b'c' as u16,
    b't' as u16,
    b' ' as u16,
    b'a' as u16,
    b' ' as u16,
    b's' as u16,
    b't' as u16,
    b'a' as u16,
    b'g' as u16,
    b'e' as u16,
    b'd' as u16,
    b' ' as u16,
    b's' as u16,
    b'a' as u16,
    b'v' as u16,
    b'e' as u16,
    b' ' as u16,
    b'p' as u16,
    b'r' as u16,
    b'o' as u16,
    b'f' as u16,
    b'i' as u16,
    b'l' as u16,
    b'e' as u16,
    b' ' as u16,
    b't' as u16,
    b'o' as u16,
    b' ' as u16,
    b'l' as u16,
    b'o' as u16,
    b'a' as u16,
    b'd' as u16,
    0,
];

const SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W: [u16; 19] = [
    b'L' as u16,
    b'o' as u16,
    b'a' as u16,
    b'd' as u16,
    b' ' as u16,
    b'S' as u16,
    b'a' as u16,
    b'v' as u16,
    b'e' as u16,
    b' ' as u16,
    b'P' as u16,
    b'r' as u16,
    b'o' as u16,
    b'f' as u16,
    b'i' as u16,
    b'l' as u16,
    b'e' as u16,
    b's' as u16,
    0,
];

const SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W: [u16; 50] = [
    b'O' as u16,
    b'p' as u16,
    b'e' as u16,
    b'n' as u16,
    b' ' as u16,
    b't' as u16,
    b'h' as u16,
    b'e' as u16,
    b' ' as u16,
    b's' as u16,
    b't' as u16,
    b'a' as u16,
    b'g' as u16,
    b'e' as u16,
    b'd' as u16,
    b' ' as u16,
    b's' as u16,
    b'a' as u16,
    b'v' as u16,
    b'e' as u16,
    b' ' as u16,
    b'f' as u16,
    b'o' as u16,
    b'l' as u16,
    b'd' as u16,
    b'e' as u16,
    b'r' as u16,
    b' ' as u16,
    b't' as u16,
    b'o' as u16,
    b' ' as u16,
    b'r' as u16,
    b'e' as u16,
    b'p' as u16,
    b'l' as u16,
    b'a' as u16,
    b'c' as u16,
    b'e' as u16,
    b' ' as u16,
    b'E' as u16,
    b'R' as u16,
    b'0' as u16,
    b'0' as u16,
    b'0' as u16,
    b'0' as u16,
    b'.' as u16,
    b's' as u16,
    b'l' as u16,
    b'2' as u16,
    0,
];

// Seamless Co-op variant of the row help above: same text but naming `ER0000.co2`, the container
// ERSC actually reads/writes. Selected at row-build time so the row never advertises the save
// flavor the active mode ignores (matches the picker's mode-locked filter; user directive
// 2026-07-06).
const SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W: [u16; 50] = [
    b'O' as u16,
    b'p' as u16,
    b'e' as u16,
    b'n' as u16,
    b' ' as u16,
    b't' as u16,
    b'h' as u16,
    b'e' as u16,
    b' ' as u16,
    b's' as u16,
    b't' as u16,
    b'a' as u16,
    b'g' as u16,
    b'e' as u16,
    b'd' as u16,
    b' ' as u16,
    b's' as u16,
    b'a' as u16,
    b'v' as u16,
    b'e' as u16,
    b' ' as u16,
    b'f' as u16,
    b'o' as u16,
    b'l' as u16,
    b'd' as u16,
    b'e' as u16,
    b'r' as u16,
    b' ' as u16,
    b't' as u16,
    b'o' as u16,
    b' ' as u16,
    b'r' as u16,
    b'e' as u16,
    b'p' as u16,
    b'l' as u16,
    b'a' as u16,
    b'c' as u16,
    b'e' as u16,
    b' ' as u16,
    b'E' as u16,
    b'R' as u16,
    b'0' as u16,
    b'0' as u16,
    b'0' as u16,
    b'0' as u16,
    b'.' as u16,
    b'c' as u16,
    b'o' as u16,
    b'2' as u16,
    0,
];

const SYSTEM_QUIT_SAVE_GAME_LABEL_W: [u16; 10] = [
    b'S' as u16,
    b'a' as u16,
    b'v' as u16,
    b'e' as u16,
    b' ' as u16,
    b'G' as u16,
    b'a' as u16,
    b'm' as u16,
    b'e' as u16,
    0,
];
const SYSTEM_QUIT_SAVE_GAME_HELP_W: [u16; 36] = [
    b'S' as u16,
    b'a' as u16,
    b'v' as u16,
    b'e' as u16,
    b' ' as u16,
    b'a' as u16,
    b'n' as u16,
    b'd' as u16,
    b' ' as u16,
    b'r' as u16,
    b'e' as u16,
    b't' as u16,
    b'u' as u16,
    b'r' as u16,
    b'n' as u16,
    b' ' as u16,
    b't' as u16,
    b'o' as u16,
    b' ' as u16,
    b'p' as u16,
    b'l' as u16,
    b'a' as u16,
    b'y' as u16,
    b'i' as u16,
    b'n' as u16,
    b'g' as u16,
    b' ' as u16,
    b't' as u16,
    b'h' as u16,
    b'e' as u16,
    b' ' as u16,
    b'g' as u16,
    b'a' as u16,
    b'm' as u16,
    b'e' as u16,
    0,
];
const SYSTEM_QUIT_SAVE_GAME_DIALOG_W: [u16; 37] = [
    b'S' as u16,
    b'a' as u16,
    b'v' as u16,
    b'e' as u16,
    b' ' as u16,
    b'a' as u16,
    b'n' as u16,
    b'd' as u16,
    b' ' as u16,
    b'r' as u16,
    b'e' as u16,
    b't' as u16,
    b'u' as u16,
    b'r' as u16,
    b'n' as u16,
    b' ' as u16,
    b't' as u16,
    b'o' as u16,
    b' ' as u16,
    b'p' as u16,
    b'l' as u16,
    b'a' as u16,
    b'y' as u16,
    b'i' as u16,
    b'n' as u16,
    b'g' as u16,
    b' ' as u16,
    b't' as u16,
    b'h' as u16,
    b'e' as u16,
    b' ' as u16,
    b'g' as u16,
    b'a' as u16,
    b'm' as u16,
    b'e' as u16,
    b'?' as u16,
    0,
];

unsafe fn wide_equals_ascii(ptr: usize, ascii: &[u8]) -> bool {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || ascii.is_empty() {
        return false;
    }
    for (idx, want) in ascii.iter().copied().enumerate() {
        let Some(unit) = (unsafe { safe_read_u16(ptr + idx * core::mem::size_of::<u16>()) }) else {
            return false;
        };
        if unit != want as u16 {
            return false;
        }
    }
    matches!(
        unsafe { safe_read_u16(ptr + ascii.len() * core::mem::size_of::<u16>()) },
        Some(0)
    )
}

pub(crate) unsafe extern "system" fn system_quit_save_game_get_and_format_hook(
    out: usize,
    getter: usize,
    text_id: i32,
    fmg_name: usize,
    abbrev: usize,
) -> usize {
    let replacement = if text_id == SYSTEM_QUIT_SAVE_GAME_MENU_TEXT_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRMT") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_LABEL_W.as_ptr() as usize)
    } else if text_id == SYSTEM_QUIT_SAVE_GAME_LINEHELP_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRHK") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_HELP_W.as_ptr() as usize)
    } else if text_id == SYSTEM_QUIT_SAVE_GAME_DIALOG_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRD") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_DIALOG_W.as_ptr() as usize)
    } else {
        None
    };
    if let Some(text_ptr) = replacement {
        match game_rva(MSG_REPOSITORY_FORMAT_RVA) {
            Ok(format_addr) => {
                let format_fn: unsafe extern "system" fn(usize, usize, u32, usize, usize) -> usize =
                    unsafe { std::mem::transmute(format_addr) };
                SYSTEM_QUIT_SAVE_GAME_TEXT_SUBSTITUTION_COUNT.fetch_add(1, Ordering::SeqCst);
                return unsafe { format_fn(out, text_ptr, text_id as u32, fmg_name, abbrev) };
            }
            Err(_) => append_autoload_debug(format_args!(
                "system-quit-save: failed to resolve MsgRepository::Format rva 0x{MSG_REPOSITORY_FORMAT_RVA:x}; forwarding id={text_id}"
            )),
        }
    }
    let orig = SYSTEM_QUIT_SAVE_GAME_GET_AND_FORMAT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return out;
    }
    let original: unsafe extern "system" fn(usize, usize, i32, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(out, getter, text_id, fmg_name, abbrev) }
}

unsafe fn system_quit_save_game_close_window(window: usize, label: &str) -> bool {
    if window < 0x10000 || window == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let vt = unsafe { safe_read_usize(window) }.unwrap_or(0);
    if vt < 0x10000 {
        append_autoload_debug(format_args!(
            "system-quit-save: skip close {label}=0x{window:x}; invalid vt=0x{vt:x}"
        ));
        return false;
    }
    let Ok(close_addr) = game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve native close rva 0x{SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA:x}; cannot close {label}=0x{window:x}"
        ));
        return false;
    };
    let close_fn: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(close_addr) };
    unsafe { close_fn(window) };
    SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-save: native cancel-close {label}=0x{window:x} vt=0x{vt:x}"
    ));
    true
}

unsafe fn system_quit_save_game_request_save_only() {
    let Ok(request_save_addr) = game_rva(SYSTEM_QUIT_REQUEST_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve RequestSave rva 0x{SYSTEM_QUIT_REQUEST_SAVE_RVA:x}"
        ));
        return;
    };
    let request_save: unsafe extern "system" fn(u8) =
        unsafe { std::mem::transmute(request_save_addr) };
    unsafe { request_save(true as u8) };
    match game_rva(SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA) {
        Ok(profile_addr) => {
            let save_request_profile: unsafe extern "system" fn(u8) =
                unsafe { std::mem::transmute(profile_addr) };
            unsafe { save_request_profile(true as u8) };
        }
        Err(_) => append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve SaveRequest_Profile rva 0x{SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA:x}; RequestSave already issued"
        )),
    }
}

unsafe fn system_quit_save_game_fire_save_and_close(dialog: usize, source: &str) -> bool {
    if dialog < 0x10000 || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "system-quit-save: {source} abort -- dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    unsafe { system_quit_save_game_request_save_only() };
    SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT.fetch_add(1, Ordering::SeqCst);
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    // `dialog` is the System/Quit tab's PropertyEditDialog, not a `MenuWindow`; calling the
    // MenuWindow cancel-close primitive on it dispatches the wrong vfunc. Close the owning menu
    // windows only, matching the Escape/back stack semantics instead of treating the row dialog as a
    // window.
    let closed_dialog = false;
    let closed_option = if option != 0 {
        unsafe { system_quit_save_game_close_window(option, "option_window") }
    } else {
        false
    };
    // Do not close the root IngameTop in the same call stack: runtime proof showed that closing the
    // full root stack immediately after the row action terminates the process. The native Escape
    // flow unwinds from the active submenu first, so close OptionSetting now and schedule IngameTop
    // for a later game-task tick.
    let closed_top = false;
    if top != 0 && top != option {
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW.store(top, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.store(2, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "system-quit-save: {source} -> save-only + native menu-window close-all dialog=0x{dialog:x} option=0x{option:x} top=0x{top:x} closed_dialog={closed_dialog} closed_option={closed_option} closed_top={closed_top}"
    ));
    closed_option || closed_top
}

pub(crate) unsafe fn system_quit_save_game_deferred_close_tick() {
    let frames = SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.load(Ordering::SeqCst);
    if frames == 0 {
        return;
    }
    if frames > 1 {
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.store(0, Ordering::SeqCst);
    let top = SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW.swap(0, Ordering::SeqCst);
    if top != 0 {
        let closed = unsafe { system_quit_save_game_close_window(top, "deferred_ingame_top_window") };
        append_autoload_debug(format_args!(
            "system-quit-save: deferred IngameTop close top=0x{top:x} closed={closed}"
        ));
    }
}

pub(crate) unsafe extern "system" fn system_quit_save_game_return_title_request_hook() {
    let dialog = SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.swap(0, Ordering::SeqCst);
    if dialog >= 0x10000 && callstack_contains_game_rva(0x7a3000, 0x7a4000) {
        unsafe { system_quit_save_game_fire_save_and_close(dialog, "legacy_confirm") };
        append_autoload_debug(format_args!(
            "system-quit-save: legacy confirmation path suppressed native return-title request dialog=0x{dialog:x}"
        ));
        return;
    }
    if dialog != 0 {
        SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.store(dialog, Ordering::SeqCst);
    }
    let orig = SYSTEM_QUIT_SAVE_GAME_RETURN_TITLE_REQUEST_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-save: return-title request trampoline unset and no active Save Game dialog; return"
        ));
        return;
    }
    let original: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
    unsafe { original() };
}

pub(crate) unsafe extern "system" fn system_quit_duplicate_add_cancel_button_hook(
    dialog: usize,
    label: usize,
    action_fn: usize,
    enabled_fn: usize,
    keyguide_fn: usize,
) -> usize {
    let orig = SYSTEM_QUIT_DUPLICATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: original AddCancelButton trampoline is unset -- fail-open return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let caller_match = callstack_contains_game_rva(
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA
            .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let before =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let ret = unsafe { original(dialog, label, action_fn, enabled_fn, keyguide_fn) };
    if caller_match {
        let after_native =
            unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
                .unwrap_or(0);
        if after_native < 0x10 {
            if SYSTEM_QUIT_NOOP_ACTION_INSTALLED.load(Ordering::SeqCst)
                != SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES
            {
                append_autoload_debug(format_args!(
                    "system-quit-dup: matched Quit Game call but cloned action hook is not installed; skipping Load Profile/Load Save Profiles rows"
                ));
                return ret;
            }
            let Ok(label_dtor_addr) = game_rva(MENU_HELP_LABEL_DTOR_RVA) else {
                append_autoload_debug(format_args!(
                    "system-quit-dup: failed to resolve MenuHelpLabelComponent dtor rva 0x{MENU_HELP_LABEL_DTOR_RVA:x}; skipping cloned rows"
                ));
                return ret;
            };
            let label_dtor: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(label_dtor_addr) };
            let mut load_label_storage =
                std::mem::MaybeUninit::<SystemQuitMenuHelpLabelScratch>::uninit();
            let load_label = load_label_storage.as_mut_ptr() as usize;
            let load_label_ok = unsafe {
                system_quit_build_static_label_component(
                    load_label,
                    &SYSTEM_QUIT_LOAD_PROFILE_LABEL_W,
                    &SYSTEM_QUIT_LOAD_PROFILE_HELP_W,
                )
            };
            let load_ret = if load_label_ok {
                let r = unsafe { original(dialog, load_label, action_fn, enabled_fn, keyguide_fn) };
                unsafe { label_dtor(load_label) };
                r
            } else {
                0
            };
            let after_load = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(0);
            let mut open_label_storage =
                std::mem::MaybeUninit::<SystemQuitMenuHelpLabelScratch>::uninit();
            let open_label = open_label_storage.as_mut_ptr() as usize;
            let open_label_ok = unsafe {
                system_quit_build_static_label_component(
                    open_label,
                    &SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W,
                    // Name the container the active mode actually replaces: ERSC sessions use
                    // ER0000.co2, vanilla ER0000.sl2 (keeps the row help consistent with the
                    // picker's mode-locked filter).
                    if crate::telemetry::seamless_coop_loaded() {
                        &SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W
                    } else {
                        &SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W
                    },
                )
            };
            let open_ret = if open_label_ok {
                let r = unsafe { original(dialog, open_label, action_fn, enabled_fn, keyguide_fn) };
                unsafe { label_dtor(open_label) };
                r
            } else {
                0
            };
            let after_open = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(0);
            let properties = dialog + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET;
            let aligned_properties = (properties + 0x7) & !0x7;
            let load_row_index = after_load.saturating_sub(1);
            let load_row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(load_row_index);
            let open_row_index = after_open.saturating_sub(1);
            let open_row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(open_row_index);
            let load_controller =
                unsafe { safe_read_usize(load_row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
            let open_controller =
                unsafe { safe_read_usize(open_row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
            let load_action = if load_controller != 0 {
                unsafe {
                    safe_read_usize(
                        load_controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                    )
                }
                .unwrap_or(0)
            } else {
                0
            };
            let open_action = if open_label_ok && open_controller != 0 {
                unsafe {
                    safe_read_usize(
                        open_controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                    )
                }
                .unwrap_or(0)
            } else {
                0
            };
            if load_action != 0 {
                SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.store(load_action, Ordering::SeqCst);
            }
            if open_action != 0 {
                SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT.store(open_action, Ordering::SeqCst);
            }
            SYSTEM_QUIT_DUPLICATE_COUNT.fetch_add(1, Ordering::SeqCst);
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_open, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-dup: added cloned Load Profile + Load Save Profiles rows dialog=0x{dialog:x} count {before}->{after_native}->{after_load}->{after_open} ret=0x{ret:x} load_ret=0x{load_ret:x} open_label_ok={open_label_ok} open_ret=0x{open_ret:x} load_row=0x{load_row:x} load_controller=0x{load_controller:x} load_action=0x{load_action:x} open_row=0x{open_row:x} open_controller=0x{open_controller:x} open_action=0x{open_action:x}"
            ));
        } else {
            append_autoload_debug(format_args!(
                "system-quit-dup: matched Quit Game call but count after native is {after_native}, not duplicating"
            ));
        }
    }
    ret
}

/// Scaleform handler CONSTRUCTOR hook (`FUN_1411a8890`, deobf 0x1411a8870). rcx = the object being
/// constructed (the 0x58 handler embedded at container+0x40), rdx = parent. Records the object as
/// live, then forwards to the original ctor (which returns the object pointer). Read-only w.r.t.
/// game state; only maintains our live-set. See SCALEFORM_HANDLER_LIVE.
pub(crate) unsafe extern "system" fn scaleform_handler_ctor_hook(
    obj: usize,
    parent: usize,
) -> usize {
    let orig = SCALEFORM_HANDLER_CTOR_ORIG.load(Ordering::SeqCst);
    SCALEFORM_HANDLER_CTORS.fetch_add(1, Ordering::SeqCst);
    if obj != 0 {
        if let Ok(mut live) = SCALEFORM_HANDLER_LIVE.lock() {
            // Cap guard: if a genuine leak fills the table, stop growing (drop tracking of the
            // oldest) so the probe can't OOM -- the double-free detection still works for recent objs.
            if live.len() >= SCALEFORM_HANDLER_LIVE_CAP {
                live.remove(0);
            }
            live.push(obj);
        }
    }
    let _ = parent;
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return obj;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(obj, parent) }
}

/// Scaleform handler inner DESTRUCTOR hook (`FUN_1411a8920`, deobf 0x1411a8900). rcx = the object.
/// If the object is in our live-set -> a normal teardown: remove it and forward to the original.
/// If it is NOT live -> a DOUBLE-FREE (the repeated-switch ProfileSelect UAF): the original would
/// walk this object's now-garbage intrusive list and crash. Log it and RETURN WITHOUT forwarding,
/// so the freed list is never dereferenced. Safe: an already-destructed object needs no second
/// teardown. This both names the bug (counter + last-obj oracle + debug line) and stops the crash.
pub(crate) unsafe extern "system" fn scaleform_handler_dtor_hook(obj: usize) {
    let orig = SCALEFORM_HANDLER_DTOR_ORIG.load(Ordering::SeqCst);
    SCALEFORM_HANDLER_DTORS.fetch_add(1, Ordering::SeqCst);
    let live = if obj == 0 {
        false
    } else if let Ok(mut set) = SCALEFORM_HANDLER_LIVE.lock() {
        if let Some(pos) = set.iter().rposition(|&a| a == obj) {
            set.swap_remove(pos);
            true
        } else {
            false
        }
    } else {
        // Lock poisoned/unavailable: fail SAFE toward forwarding (treat as live) so we never skip a
        // legitimate destructor on a lock hiccup -- the crash is rarer than the lock being fine.
        true
    };
    if !live {
        let n = SCALEFORM_HANDLER_DOUBLE_FREES.fetch_add(1, Ordering::SeqCst) + 1;
        SCALEFORM_HANDLER_LAST_DOUBLE_FREE_OBJ.store(obj, Ordering::SeqCst);
        if n <= 32 {
            let parent = unsafe { safe_read_usize(obj + 0x18) }.unwrap_or(0);
            let list_head = unsafe { safe_read_usize(obj + 0x28) }.unwrap_or(0);
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            append_crash_log(format_args!(
                "scaleform-handler-guard: DOUBLE-FREE #{n} of handler obj=0x{obj:x} container=0x{:x} parent(+0x18)=0x{parent:x} list_head(+0x28)=0x{list_head:x} quickload_phase={phase} -- SKIPPED inner dtor (would have walked freed list) to prevent the ProfileSelect UAF crash",
                obj.wrapping_sub(0x40)
            ));
        }
        return;
    }
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { f(obj) };
}
