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
pub(crate) use er_quit_menu_core::profile_load_dialog::{
    system_quit_open_profile_load_dialog, system_quit_open_profile_load_dialog_on,
};

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

/// Native cancel-close of one menu window. `pub(crate)` because the save-flow tick closes the
/// destination browser through the same primitive the deferred IngameTop close uses.
pub(crate) unsafe fn system_quit_save_game_close_window(window: usize, label: &str) -> bool {
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

pub(crate) unsafe fn system_quit_save_game_request_save_only() {
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

/// Fire the native save request pair FORCED: `throttled = false` for both calls.
///
/// The bool is pinned (1.16.2 Ghidra decompile, 2026-07-28; see the RVA consts):
/// `RequestSave(true)` runs a 60-second throttle against `GameMan+0xb98` whose early
/// return sets no flags, and `SaveRequest_Profile(true)` the same against `+0xb88`
/// (`SetSeconds(0x3c)`). Under boot-time global suppression the dispatchers still run
/// their commit tails for swallowed autosaves, so those throttle timestamps stay warm --
/// a throttled user press within 60 s of any swallowed autosave would silently set
/// nothing and strand the armed one-shot bypass token. The Save Game commit therefore
/// always fires with `false`. The quit-to-desktop sites deliberately keep
/// `system_quit_save_game_request_save_only` (true/true): under suppression those become
/// intentional no-op saves -- the Save Game row is the only path that really writes.
pub(crate) unsafe fn system_quit_save_game_request_save_forced() {
    const FORCED_NOT_THROTTLED: u8 = false as u8;
    let Ok(request_save_addr) = game_rva(SYSTEM_QUIT_REQUEST_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "save-flow: failed to resolve RequestSave rva 0x{SYSTEM_QUIT_REQUEST_SAVE_RVA:x}; forced save NOT fired"
        ));
        return;
    };
    let request_save: unsafe extern "system" fn(u8) =
        unsafe { std::mem::transmute(request_save_addr) };
    unsafe { request_save(FORCED_NOT_THROTTLED) };
    match game_rva(SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA) {
        Ok(profile_addr) => {
            let save_request_profile: unsafe extern "system" fn(u8) =
                unsafe { std::mem::transmute(profile_addr) };
            unsafe { save_request_profile(FORCED_NOT_THROTTLED) };
        }
        Err(_) => append_autoload_debug(format_args!(
            "save-flow: failed to resolve SaveRequest_Profile rva 0x{SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA:x}; forced RequestSave already issued"
        )),
    }
}

/// Call one of the game's own save-request retractions after verifying its whole body.
///
/// Fails closed through `save_flow_verify_rva`: an unresolvable address or a single drifted
/// byte skips the call and reports it. Not retracting costs CPU; calling unknown code costs
/// the process.
pub(crate) unsafe fn call_verified_retract(
    rva: u32,
    expected: &[u8],
    mask: &[u8],
    name: &str,
) -> bool {
    let Some(address) = save_flow_verify_rva(rva, expected, mask, name) else {
        return false;
    };
    let retract: unsafe extern "system" fn() = unsafe { std::mem::transmute(address) };
    unsafe { retract() };
    true
}

/// Retract the two native save-request flags through the game's own setters.
///
/// `b72` / `b73` select which flags to clear -- the caller decides ownership; this only
/// performs it. Returns the pair of "actually cleared" results.
pub(crate) unsafe fn system_quit_save_request_retract(b72: bool, b73: bool) -> (bool, bool) {
    let cleared_b72 = b72
        && unsafe {
            call_verified_retract(
                SAVE_REQUEST_RETRACT_B72_RVA,
                SAVE_REQUEST_RETRACT_B72_SIG,
                SAVE_REQUEST_RETRACT_B72_SIG_MASK,
                "b72",
            )
        };
    let cleared_b73 = b73
        && unsafe {
            call_verified_retract(
                SAVE_REQUEST_RETRACT_B73_RVA,
                SAVE_REQUEST_RETRACT_B73_SIG,
                SAVE_REQUEST_RETRACT_B73_SIG_MASK,
                "b73",
            )
        };
    (cleared_b72, cleared_b73)
}

/// Run the proven Save Game close-all sequence and hand the flow to `save_flow_tick`.
///
/// `commit` selects the destination stage: `true` = stage 6 CLOSING_COMMIT (the tick fires
/// the forced save request once the menus are closed and the RAM gates are green), `false` =
/// stage 5 CLOSING_ABORT (the user declined; the tick returns to the world having written
/// nothing).
///
/// Close-then-fire (save-game-flow WP1, 2026-07-28): the save request is never fired here.
/// Firing while menus are open is a dispatch-split hazard -- `ShouldSave` (the b72 lane)
/// requires `!CanShowSaveMenu()` (`CSMenuMan+0x13c == 0`) but the b73 gate `FUN_140679370`
/// does not, so an open-menu fire lets the b73-only lane dispatch a system-only submit first,
/// that submit consumes the one-shot suppression-bypass token, and the later char-slot submit
/// is swallowed. Staging the commit and firing at stage 7 produces a single combined
/// `b72 && b73` -> `FUN_14067b940` -> one `FUN_140e6ef60` submit -> one enqueue -> one token.
///
/// WP1/WP2 commit plan: overwrite the loaded save (WP3 adds a destination target).
pub(crate) unsafe fn system_quit_save_game_close_menus(
    dialog: usize,
    source: &str,
    commit: bool,
) -> bool {
    if dialog < 0x10000 || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "system-quit-save: {source} abort -- dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    // Stage the outcome before the close sequence so the save-flow tick owns the flow
    // from the next game-task frame onward.
    SAVE_FLOW_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    SAVE_FLOW_STAGE.store(
        if commit {
            SAVE_FLOW_STAGE_CLOSING_COMMIT
        } else {
            SAVE_FLOW_STAGE_CLOSING_ABORT
        },
        Ordering::SeqCst,
    );
    if commit {
        SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT.fetch_add(1, Ordering::SeqCst);
    }
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
        "system-quit-save: {source} -> staged {} + native menu-window close-all dialog=0x{dialog:x} option=0x{option:x} top=0x{top:x} closed_dialog={closed_dialog} closed_option={closed_option} closed_top={closed_top}",
        if commit {
            "CLOSING_COMMIT (close-then-fire)"
        } else {
            "CLOSING_ABORT (nothing will be written)"
        }
    ));
    closed_option || closed_top
}

/// Enter the Save Game flow from the row press: Straight to the destination list.
///
/// Captures the System/Quit dialog the whole flow is anchored on, then hands the browser open to
/// the menu pump via `SAVE_DEST_OPEN_PICKER_PENDING` and parks in stage 3. Nothing is asked here
/// and nothing is written here.
///
/// Why the pump and not an inline open. Opening the destination browser stages ProfileSummary row
/// records and submits an `05_010` MenuJob; `system_quit_menu_window_run_post` is the context that
/// was runtime-proven to own that submit (2026-07-29, three consecutive commits), and it is the
/// same hand-off the OS surface needs in order to block inside comdlg32 off the row-action stack.
/// The row press therefore stages the request rather than performing it, and stage 3's
/// `SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS` bounds a pump that never picks it up.
///
/// There is no "DEGRADE TO AN IMMEDIATE COMMIT" path any more, and its removal is a safety fix
/// rather than a simplification. It existed because the old flow could not ask its first question
/// without the MessageBoxBuilder recipe, so a build that failed the prologue check wrote to the
/// loaded save with no confirm at all. Opening a list needs no message box, so a broken recipe now
/// costs only the overwrite confirm -- and an unconfirmable overwrite is refused at the pick
/// (`save_dest_handle_picked_target`), never performed silently. A free destination name still
/// commits, because it never needed a confirm in the first place.
pub(crate) unsafe fn system_quit_save_game_start_flow(dialog: usize) -> bool {
    if dialog < 0x10000 || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-flow: row press abort -- dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    save_flow_box_clear();
    // Defensive: no destination state may survive from an earlier flow (a stale target would send
    // this save to the wrong file).
    save_dest_reset("save game row press");
    SAVE_FLOW_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    // The overwrite confirm is only answerable if the MessageBoxDialog builder hook is live --
    // that detour is what captures the dialog pointer the stage machine polls. It is normally
    // installed at boot (`online_disable_enabled()` path in the game task), but make sure:
    // this call is idempotent, and the row press is the menu thread, i.e. the one context in
    // which no other thread can be executing the builder while MinHook patches it.
    install_auto_accept_hook();
    // Same reasoning for the answer observer: `CS::MenuJob::EmitResult` is what tells us which
    // button the user pressed on the branch where the dialog never stores its result. Install
    // is idempotent; if it is not live the poll falls back to `dialog+0x1e8` and reports
    // UNDECIDABLE rather than guessing, so log the state here where the run can see it.
    install_menu_job_emit_result_hook();
    let capture_live = MSGBOX_BUILDER_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET;
    // One press = one bypass arm = one commit. Counting presses is what makes
    // `oracle_save_bypass_allowed_total = 2` readable as "two presses" instead of "a double-arm
    // bug"; without it the two are indistinguishable from telemetry alone.
    let press = SAVE_FLOW_ROW_PRESS_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "save-flow: row press #{press} dialog=0x{dialog:x} -> opening the destination list (no confirm is asked here); overwrite_confirm_available={} builder_capture_live={capture_live} emit_observer_installed={}",
        save_flow_box_recipe_available(),
        MENU_JOB_EMIT_RESULT_INSTALLED.load(Ordering::SeqCst)
    ));
    if !save_flow_box_recipe_available() || !capture_live {
        // Not fatal to the press, and deliberately so: the list needs no message box. Only a pick
        // that would clobber an existing file needs one, and that pick is refused rather than
        // written blind. Say it once here so a run can attribute a later refusal.
        append_autoload_debug(format_args!(
            "save-flow: the overwrite confirm cannot be built on this build (recipe_ok={} builder_capture_live={capture_live}) -- the destination list still opens; picking an EXISTING file will be refused, picking a free name still commits",
            save_flow_box_recipe_available()
        ));
    }
    // Menu-pump owns the open (see this function's docs). Stage 3 waits for it and bounds it.
    SAVE_DEST_OPEN_PICKER_PENDING.store(1, Ordering::SeqCst);
    SAVE_FLOW_STAGE.store(SAVE_FLOW_STAGE_DEST_BROWSE, Ordering::SeqCst);
    true
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
        let closed =
            unsafe { system_quit_save_game_close_window(top, "deferred_ingame_top_window") };
        append_autoload_debug(format_args!(
            "system-quit-save: deferred IngameTop close top=0x{top:x} closed={closed}"
        ));
    }
}

/// A 1.16.2-only stack band, and one that cannot be carried forward.
///
/// The pair below is a 4 KB window of `.text`, not a function plus an offset, so there is nothing
/// for the 1.16.2 -> 1.17 map to key on. That is not a gap in the map; it is a property of the
/// band. Measured across the 33 `.pdata` functions it overlaps: 12 are unmapped outright and the
/// 21 that map move by seven different deltas (`+0xdf0`, `+0xe20`, `+0xe30`, `+0xe40`, `+0xe80`,
/// `+0xe90`, `+0x1560`). A band whose contents move apart has no translated width, so no anchor
/// rescues it -- unlike the GX transport band, whose 12 functions all move `+0x1e00` together and
/// which is therefore anchored rather than refused.
///
/// It is also the weakest-evidenced comparison in the tree. `SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA`
/// (`0x67a3a0`) has exactly one direct caller in the whole 1.16.2 image, at `0x59d90e` inside
/// `FUN_14059d8b0`, which is nowhere near this band; nothing records what the band was measured
/// from. So the honest treatment is to decline on any build it was not measured on and say so,
/// rather than invent a 1.17 window. The branch it guards is documented dormant -- the product row
/// path clears the arming latch before this hook runs -- so declining costs a safety net that
/// nothing currently reaches, and 1.16.2 behaviour is unchanged.
const LEGACY_CONFIRM_CALLER_BAND: core::ops::Range<usize> = 0x7a3000..0x7a4000;

/// Say once, and at most a handful of times, that a build comparison has no answer on this build.
///
/// The same shape `er_quit_menu_core::row_cloner` uses for the row-return addresses: a refusal that
/// is loud, bounded, and names the constant rather than going quiet.
fn note_unsupported_build_comparison(what: &str) {
    use std::sync::atomic::AtomicUsize;
    static SAID: AtomicUsize = AtomicUsize::new(0);
    const SAY_AT_MOST: usize = 8;
    if SAID.fetch_add(1, Ordering::SeqCst) < SAY_AT_MOST {
        append_autoload_debug(format_args!(
            "system-quit: {what} cannot be compared on {}; treating it as no match",
            er_game_base::game_build::describe_build()
        ));
    }
}

pub(crate) unsafe extern "system" fn system_quit_save_game_return_title_request_hook() {
    let dialog = SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.swap(0, Ordering::SeqCst);
    let legacy_confirm_caller = if er_game_base::game_build::is_supported_build() {
        callstack_contains_game_rva(
            LEGACY_CONFIRM_CALLER_BAND.start,
            LEGACY_CONFIRM_CALLER_BAND.end,
        )
    } else {
        note_unsupported_build_comparison(
            "the legacy Save Game confirm caller band (0x7a3000..0x7a4000, untranslatable)",
        );
        false
    };
    if dialog >= 0x10000 && legacy_confirm_caller {
        // Dormant safety net (the product row path clears the arming latch). It keeps the
        // WP1 semantics -- close-then-fire straight to the commit, no confirm chain.
        unsafe { system_quit_save_game_close_menus(dialog, "legacy_confirm", true) };
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
    if obj != 0
        && let Ok(mut live) = SCALEFORM_HANDLER_LIVE.lock()
    {
        // Cap guard: if a genuine leak fills the table, stop growing (drop tracking of the
        // oldest) so the probe can't OOM -- the double-free detection still works for recent objs.
        if live.len() >= SCALEFORM_HANDLER_LIVE_CAP {
            live.remove(0);
        }
        live.push(obj);
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
/// If it is not live -> a double-free (the repeated-switch ProfileSelect UAF): the original would
/// walk this object's now-garbage intrusive list and crash. Log it and return without forwarding,
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
        // Lock poisoned/unavailable: fail safe toward forwarding (treat as live) so we never skip a
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
