
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

fn system_quit_native_return_visual_fallback_row(cursor: i32) -> Option<i32> {
    if cursor == 2 || cursor == 3 {
        return Some(cursor);
    }
    // The patched native GameEnd movie has four visible buttons, but its original click dispatcher can
    // still report the native Return-to-Desktop action/cursor for the lower cloned visuals. For mouse
    // use, disambiguate by the same OS cursor position the game polls: bottom-left is Load Profile,
    // bottom-right is Load Save Profiles. Top row and unknown cursor stay native Return to Desktop.
    let Some((nx, ny)) = read_cursor_normalized() else {
        return None;
    };
    if ny > 0.12 {
        if nx < 0.0 { Some(2) } else { Some(3) }
    } else {
        None
    }
}

unsafe fn system_quit_route_button_action_or_forward(
    action_obj: usize,
    orig: usize,
    hook_name: &str,
) -> usize {
    let open_save_dir_action = SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT.load(Ordering::SeqCst);
    if action_obj != 0 && action_obj == open_save_dir_action {
        SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = unsafe { system_quit_open_save_picker_menu(action_obj) };
        append_autoload_debug(format_args!(
            "system-quit-open-save-dir: cloned action selected action=0x{action_obj:x} opened={opened} (in-game save picker); suppressing native Quit Game row action"
        ));
        return 0;
    }
    let recorded_action = SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.load(Ordering::SeqCst);
    let native_return_desktop_action =
        SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_ACTION_LAST_OBJECT.load(Ordering::SeqCst);
    let dialog = unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(0);
    let cursor = if dialog >= 0x10000 {
        unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    let native_return_visual_row = if action_obj != 0 && action_obj == native_return_desktop_action {
        system_quit_native_return_visual_fallback_row(cursor)
    } else {
        None
    };
    if action_obj != 0
        && (action_obj == recorded_action || native_return_visual_row == Some(2))
    {
        let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
        if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
            append_autoload_debug(format_args!(
                "system-quit-dup: cloned quick-load action re-entry ignored action=0x{action_obj:x} phase={phase}; native handoff already armed"
            ));
            return 0;
        }
        SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = unsafe { system_quit_open_profile_load_dialog(action_obj) };
        let mouse = read_cursor_normalized();
        append_autoload_debug(format_args!(
            "system-quit-dup: Load Profile action selected action=0x{action_obj:x} cursor={cursor} native_visual_row={:?} mouse={:?} recorded_action=0x{recorded_action:x} native_return_action=0x{native_return_desktop_action:x} opened={opened}; suppressing native Quit Game row action until ProfileSelect confirms slot",
            native_return_visual_row, mouse
        ));
        return 0;
    }
    if action_obj != 0 && native_return_visual_row == Some(3) {
        SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = unsafe { system_quit_open_save_picker_menu(action_obj) };
        let mouse = read_cursor_normalized();
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: Load Save Profiles action selected via native-return action=0x{action_obj:x} cursor={cursor} native_visual_row={:?} mouse={:?} opened={opened} (in-game save picker); suppressing native Quit Game row action",
            native_return_visual_row, mouse
        ));
        return 0;
    }
    let save_game_action = SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT.load(Ordering::SeqCst);
    if action_obj != 0 && action_obj == save_game_action {
        let dialog = unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(0);
        if dialog >= 0x10000 {
            SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let closed = unsafe { system_quit_save_game_fire_save_and_close(dialog, "row_action") };
            append_autoload_debug(format_args!(
                "system-quit-save: Save Game native row selected action=0x{action_obj:x} dialog=0x{dialog:x}; requested save + close-all closed={closed}; suppressed native Quit Game/return-title action"
            ));
            return 0;
        }
    }
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-save: {hook_name} action trampoline is unset for action=0x{action_obj:x} dialog=0x{dialog:x}; fail-open return 0"
        ));
        return 0;
    }
    append_autoload_debug(format_args!(
        "system-quit-save: {hook_name} original native row selected action=0x{action_obj:x} dialog=0x{dialog:x}; forwarding native action"
    ));
    let original: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { original(action_obj) }
}

pub(crate) unsafe extern "system" fn system_quit_noop_desktop_action_hook(
    action_obj: usize,
) -> usize {
    let orig = SYSTEM_QUIT_NOOP_ACTION_ORIG.load(Ordering::SeqCst);
    unsafe { system_quit_route_button_action_or_forward(action_obj, orig, "save-game/first-row") }
}

pub(crate) unsafe extern "system" fn system_quit_return_desktop_action_hook(
    action_obj: usize,
) -> usize {
    let orig = SYSTEM_QUIT_RETURN_DESKTOP_ACTION_ORIG.load(Ordering::SeqCst);
    unsafe { system_quit_route_button_action_or_forward(action_obj, orig, "return-desktop/second-row") }
}

unsafe fn system_quit_forward_button_controller_activation(
    controller: usize,
    event_kind: u32,
    event_a: usize,
    event_b: usize,
) {
    let orig = PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: PropertyNewButtonController activation trampoline unset for controller=0x{controller:x}; fail-closed return"
        ));
        return;
    }
    let original: unsafe extern "system" fn(usize, u32, usize, usize) = unsafe { std::mem::transmute(orig) };
    unsafe { original(controller, event_kind, event_a, event_b) };
}

unsafe fn system_quit_controller_should_invoke_action(controller: usize, event_a: usize) -> bool {
    let Ok(predicate_addr) = game_rva(PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve PropertyNewButtonController action predicate rva 0x{PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA:x}; forwarding native activation"
        ));
        return false;
    };
    let predicate: unsafe extern "system" fn(usize, usize) -> u8 = unsafe { std::mem::transmute(predicate_addr) };
    unsafe { predicate(controller, event_a) != 0 }
}

pub(crate) unsafe extern "system" fn property_new_button_controller_activate_hook(
    controller: usize,
    event_kind: u32,
    event_a: usize,
    event_b: usize,
) {
    let load_controller = SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst);
    let open_controller = SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst);
    let custom_controller = controller != 0 && (controller == load_controller || controller == open_controller);
    if custom_controller
        && !unsafe { system_quit_controller_should_invoke_action(controller, event_a) }
    {
        unsafe {
            system_quit_forward_button_controller_activation(controller, event_kind, event_a, event_b)
        };
        return;
    }
    if controller != 0 && controller == load_controller {
        let action = unsafe {
            safe_read_usize(controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET)
        }
        .unwrap_or(0);
        let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
        if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
            append_autoload_debug(format_args!(
                "system-quit-dup: controller quick-load activation ignored controller=0x{controller:x} action=0x{action:x} phase={phase}; native handoff already armed"
            ));
            return;
        }
        SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = if action != 0 {
            unsafe { system_quit_open_profile_load_dialog(action) }
        } else {
            false
        };
        append_autoload_debug(format_args!(
            "system-quit-dup: Load Profile controller selected controller=0x{controller:x} action=0x{action:x} event_kind={event_kind} event_a=0x{event_a:x} event_b=0x{event_b:x} opened={opened}; suppressing native button activation"
        ));
        return;
    }
    if controller != 0 && controller == open_controller {
        let action = unsafe {
            safe_read_usize(controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET)
        }
        .unwrap_or(0);
        SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = if action != 0 {
            unsafe { system_quit_open_save_picker_menu(action) }
        } else {
            false
        };
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: Load Save Profiles controller selected controller=0x{controller:x} action=0x{action:x} event_kind={event_kind} event_a=0x{event_a:x} event_b=0x{event_b:x} opened={opened} (in-game save picker); suppressing native button activation"
        ));
        return;
    }
    unsafe { system_quit_forward_button_controller_activation(controller, event_kind, event_a, event_b) };
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

/// Validate + ingest a picked save container path (any picker UI feeds this): mode-locked
/// extension, BND4 parse, SteamID normalization, ProfileSummary slot preview, candidate staging,
/// and last-picked-directory persistence. Menu-thread only (preview writes + renderer refresh).
/// The caller is responsible for the pre-pick work (`system_quit_save_swap_restore_profile_summary`
/// + `system_quit_save_swap_arm_original`), which happens at picker OPEN time.
unsafe fn system_quit_ingest_picked_save(selected_path: &str) -> bool {
    // Mode lock: only the container flavor the active runtime loads. A cross-flavor save (`.sl2`
    // while Seamless owns the session, `.co2` in vanilla) would preview character slots the
    // active runtime never actually loads (mixing save flavors across modes corrupts
    // expectations; user directive 2026-07-06).
    let seamless = save_picker_seamless_mode_after_settle("system-quit-ingest-picked-save");
    let picker_ext = if seamless { "co2" } else { "sl2" };
    let selected_log = system_quit_windows_path_for_log(selected_path);
    if !Path::new(selected_path).is_file() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected path is not a file '{}'",
            selected_log
        ));
        return false;
    }
    let ext_ok = Path::new(selected_path)
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
    let Ok(mut bytes) = fs::read(selected_path) else {
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
    if crate::config::autoupdate_preferred_picker_dir_enabled()
        && let Some(parent) = Path::new(selected_path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
    {
        crate::config::remember_preferred_save_picker_dir(parent);
    }
    SYSTEM_QUIT_OPEN_SAVE_DIR_SUCCESS_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-load-save-profiles: applied selected save preview '{}' len={len} hash=0x{hash:016x} slot_mask=0x{mask:x}; staged active save unchanged until a foreign slot is selected",
        selected_log
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
    let replacement = if text_id == SYSTEM_QUIT_FIRST_ROW_MENU_TEXT_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRMT") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_LABEL_W.as_ptr() as usize)
    } else if text_id == SYSTEM_QUIT_FIRST_ROW_LINEHELP_ID
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
    let first_row_call = callstack_contains_game_rva(
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA
            .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let second_row_call = callstack_contains_game_rva(
        SYSTEM_QUIT_SECOND_ROW_TARGET_RETURN_RVA
            .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        SYSTEM_QUIT_SECOND_ROW_TARGET_RETURN_RVA + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let before =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let ret = unsafe { original(dialog, label, action_fn, enabled_fn, keyguide_fn) };
    if !(first_row_call || second_row_call) {
        return ret;
    }

    // OptionSetting constructs/lazily rebuilds hidden tab panes while another tab is visible. Mutate
    // only the Quit tab's own dialog; never write rows into the active non-Quit pane.
    let active_tab = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
    let active_dialog = OPTIONSETTING_CURRENT_DIALOG.load(Ordering::SeqCst);
    let actively_shown = OPTIONSETTING_ACTIVELY_SHOWN.load(Ordering::SeqCst) != 0;
    if actively_shown && active_tab != OPTIONSETTING_QUIT_TAB_INDEX && active_dialog == dialog {
        let skip_n = SYSTEM_QUIT_DUPLICATE_COUNT.load(Ordering::SeqCst);
        if skip_n < 16 {
            append_autoload_debug(format_args!(
                "system-quit-dup: matched Quit Game AddCancelButton but target is active non-Quit tab={active_tab} dialog=0x{dialog:x}; skipping row routing so active tab stays vanilla"
            ));
        }
        return ret;
    }

    let after_native =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let properties = dialog + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET;
    let aligned_properties = (properties + 0x7) & !0x7;
    let mut after_final = after_native;
    if after_native > before {
        let native_row_index = after_native.saturating_sub(1);
        let native_row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(native_row_index);
        let native_controller =
            unsafe { safe_read_usize(native_row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
        let native_action = if native_controller != 0 {
            unsafe {
                safe_read_usize(native_controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET)
            }
            .unwrap_or(0)
        } else {
            0
        };
        if native_action != 0 && first_row_call {
            SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT.store(native_action, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-dup: captured native first Quit row action=0x{native_action:x}; routing this in-place button to Save Game"
            ));
        } else if native_action != 0 && second_row_call {
            SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_ACTION_LAST_OBJECT.store(native_action, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-dup: captured native second Quit row action=0x{native_action:x}; leaving cursor=1 Return to Desktop native, cursor=2/3 route to custom rows if GameEnd dispatch collapses"
            ));
        }
    }

    if second_row_call {
        let Ok(label_dtor_addr) = game_rva(MENU_HELP_LABEL_DTOR_RVA) else {
            append_autoload_debug(format_args!(
                "system-quit-dup: failed to resolve MenuHelpLabelComponent dtor rva 0x{MENU_HELP_LABEL_DTOR_RVA:x}; cannot add Load Profile/Load Save Profiles rows"
            ));
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_native, Ordering::SeqCst);
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
        let (load_ret, load_row, load_controller, load_action) = if load_label_ok {
            let r = unsafe { original(dialog, load_label, action_fn, enabled_fn, keyguide_fn) };
            unsafe { label_dtor(load_label) };
            after_final = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(after_native);
            let row_index = after_final.saturating_sub(1);
            let row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(row_index);
            let controller =
                unsafe { safe_read_usize(row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
            let action = if controller != 0 {
                unsafe {
                    safe_read_usize(controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET)
                }
                .unwrap_or(0)
            } else {
                0
            };
            if action != 0 {
                SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.store(action, Ordering::SeqCst);
            }
            if controller != 0 {
                SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT.store(controller, Ordering::SeqCst);
            }
            (r, row, controller, action)
        } else {
            (0, 0, 0, 0)
        };

        let mut open_label_storage =
            std::mem::MaybeUninit::<SystemQuitMenuHelpLabelScratch>::uninit();
        let open_label = open_label_storage.as_mut_ptr() as usize;
        let open_label_ok = unsafe {
            system_quit_build_static_label_component(
                open_label,
                &SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W,
                if crate::telemetry::seamless_coop_loaded() {
                    &SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W
                } else {
                    &SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W
                },
            )
        };
        let (open_ret, open_row, open_controller, open_action) = if open_label_ok {
            let r = unsafe { original(dialog, open_label, action_fn, enabled_fn, keyguide_fn) };
            unsafe { label_dtor(open_label) };
            after_final = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(after_final);
            let row_index = after_final.saturating_sub(1);
            let row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(row_index);
            let controller =
                unsafe { safe_read_usize(row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
            let action = if controller != 0 {
                unsafe {
                    safe_read_usize(controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET)
                }
                .unwrap_or(0)
            } else {
                0
            };
            if action != 0 {
                SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT.store(action, Ordering::SeqCst);
            }
            if controller != 0 {
                SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT.store(controller, Ordering::SeqCst);
            }
            (r, row, controller, action)
        } else {
            (0, 0, 0, 0)
        };
        if load_label_ok || open_label_ok {
            SYSTEM_QUIT_DUPLICATE_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        let prior_bound = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
        let new_bound = (after_final.min(i32::MAX as usize)) as i32;
        if new_bound > prior_bound {
            unsafe { *((dialog + DIALOG_SLOT_BOUND_B08_OFFSET) as *mut i32) = new_bound };
        }
        let bound_after = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
        append_autoload_debug(format_args!(
            "system-quit-dup: added native GameEnd rows Load Profile + Load Save Profiles dialog=0x{dialog:x} count {before}->{after_native}->{after_final} cursor_bound {prior_bound}->{bound_after} ret=0x{ret:x} load_ok={load_label_ok} load_ret=0x{load_ret:x} load_row=0x{load_row:x} load_controller=0x{load_controller:x} load_action=0x{load_action:x} open_ok={open_label_ok} open_ret=0x{open_ret:x} open_row=0x{open_row:x} open_controller=0x{open_controller:x} open_action=0x{open_action:x}"
        ));
    }

    SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
    SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_final, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: routed native Quit rows dialog=0x{dialog:x} first_row_call={first_row_call} second_row_call={second_row_call} count {before}->{after_native}->{after_final}; native GameEnd GFx component preserved"
    ));
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
