use super::*;

// Row text facade. The fixed-capacity wide label/help buffers, the compile-time widener behind
// them, the two live build-url help lines and the Wine path-spelling helpers moved verbatim to
// `er_quit_menu_core::row_text`; they touch no game state, so they crossed with no seam entry.
pub(crate) use er_quit_menu_core::row_text::*;

// The two `05_010_ProfileSelect` openers moved to `er_quit_menu_core::profile_load_dialog`, which
// the standalone character-row shell links. Pure code reorganization, no behavior change: every
// symbol they read was already in a shared crate (`er-title-flow` for the three native addresses
// and the two dialog offsets, `er-telemetry-core` for the four latches), so the move added no
// `QuitMenuHost` field. Both product callers -- the Load Character row action and the save flow's
// destination browser, which opens from a captured dialog and has no row action object -- keep the
// names they always used.
pub(crate) use er_quit_menu_core::profile_load_dialog::system_quit_open_profile_load_dialog;

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

/// The active save file the character-switch feature snapshots + restores + writes to. Resolved from
/// runtime ground truth via `active_save_file_for_system_quit()`: a direct-file save selected in the
/// missing-save picker is a read-only source copied into the private redirected native save tree, so
/// this returns the game's native `%APPDATA%/EldenRing/<steamid>/ER0000.{co2|sl2}` path for writes.
/// Explicit/default saves keep using the normal configured/default resolver. Never write back to the
/// direct source file under `save-files/` or a user-picked path.
pub(crate) fn system_quit_env_save_path() -> Result<String, &'static str> {
    let Some(path) = active_save_file_for_system_quit() else {
        return Err(
            "no active save file (direct/configured save unset and no default ER0000 save resolved)",
        );
    };
    let path = path.to_string_lossy();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("resolved active save file is blank");
    }
    Ok(trimmed.trim_end_matches(['/', '\\']).to_owned())
}

pub(crate) fn system_quit_env_save_dir() -> Result<String, &'static str> {
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

/// Validate + ingest a picked save container path (any picker UI feeds this): runtime-flavor
/// extension filter, BND4 parse, SteamID normalization, ProfileSummary slot preview, candidate
/// staging, and last-picked-directory persistence. Menu-thread only (preview writes + renderer
/// refresh).
/// The caller is responsible for the pre-pick work (`system_quit_save_swap_restore_profile_summary`
/// + `system_quit_save_swap_arm_original`), which happens at picker open time.
pub(crate) unsafe fn system_quit_ingest_picked_save(selected_path: &str) -> bool {
    // Extension policy: vanilla only accepts `.sl2`; Seamless accepts its native `.co2` plus
    // vanilla `.sl2` sources so a vanilla save can be loaded/imported while ERSC owns the session.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-ingest-picked-save");
    let allowed_exts: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    let selected_log = system_quit_windows_path_for_log(selected_path);
    if !Path::new(selected_path).is_file() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(er_save_picker_core::PickerStatusMessage::new(
            "SAVE NOT FOUND",
            "The selected path is missing or is not a file.",
        ));
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected path is not a file '{}' (visible reason set)",
            selected_log
        ));
        return false;
    }
    // The shared extension filter (`save_picker.rs`), not a second copy: the in-game listing, the
    // OS dialog's post-return check and this ingest gate must not be able to disagree about which
    // container flavors the active runtime accepts.
    let ext_ok = crate::experiments::save_picker::save_picker_extension_accepted(
        Path::new(selected_path),
        allowed_exts,
    );
    if !ext_ok {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker_core::PickRejection::WrongExtension
                .status_message(&allowed_exts.join("/.")),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: rejected '{}' -- picker accepts only .{} (seamless={seamless}; visible reason set)",
            selected_log,
            allowed_exts.join("/.")
        ));
        return false;
    }
    let Ok(mut bytes) = fs::read(selected_path) else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker_core::PickRejection::Unreadable.status_message("SL2"),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: failed to read selected save '{}' (visible reason set)",
            selected_log
        ));
        return false;
    };
    let len = bytes.len() as u64;
    let raw_hash = system_quit_hash_bytes(&bytes);
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker_core::PickRejection::NotBnd4.status_message("SL2"),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save is not a valid BND4 '{}' len={len} hash=0x{raw_hash:016x} (visible reason set)",
            selected_log
        ));
        return false;
    }
    let Ok(base) = game_module_base() else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(er_save_picker_core::PickerStatusMessage::new(
            "GAME STATE NOT READY",
            "The save is valid, but the game was not ready to preview it.",
        ));
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save '{}' is valid but game module base is unavailable (visible reason set)",
            selected_log
        ));
        return false;
    };
    normalize_save_bytes_to_active_steam_id(base, &mut bytes, "system-quit-picker-selection");
    let hash = system_quit_hash_bytes(&bytes);
    let mask = unsafe { system_quit_apply_foreign_profile_summary_preview(base, &bytes) };
    if mask == 0 {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        save_picker_set_visible_status(
            er_save_picker_core::PickRejection::NoLoadableCharacter.status_message("SL2"),
        );
        append_autoload_debug(format_args!(
            "system-quit-load-save-profiles: selected save had no readable character slots '{}' len={len} hash=0x{hash:016x} (visible reason set)",
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

pub(crate) unsafe fn wide_equals_ascii(ptr: usize, ascii: &[u8]) -> bool {
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
