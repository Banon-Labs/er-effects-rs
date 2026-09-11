use super::*;
use er_game_base::fnv1a::{FNV1A64_OFFSET_BASIS, fnv1a64};

/// Install the row-populate hook (`FUN_1408758d0`). Runs at most once per process -- the claim is
/// the first statement -- and mirrors the named-child binder install.
pub(crate) fn install_profile_row_populate_hook() {
    // One claim, not a check-then-act read of the success latches -- this installer has two owners
    // (`title_visual_startup.rs:31` synchronously, then `:37` on the thread that call spawns), and
    // every latch it could consult instead is stored only after its own apply succeeds. See
    // `PROFILE_ROW_POPULATE_CLAIMED`, and `TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_CLAIMED` for the
    // sibling that was measured racing rather than merely able to.
    if PROFILE_ROW_POPULATE_CLAIMED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "stats-text: row-populate MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    // Four independent rows, each skipping only itself (2026-08-30). Every block below used a
    // bare `return` for its own refusal, so one unmapped RVA on 1.17 took the remaining rows with
    // it -- e.g. a refused player-name getter also cost the ProfileSelect row-populate and the
    // row-model builder, which are unrelated functions serving unrelated rows. A labelled block
    // per row keeps a refusal local. bd `one-refused-hook-must-not-abort-the-installer-2026-08-30`.
    'name_getter: {
        if PLAYER_GAME_DATA_NAME_GETTER_INSTALLED.load(Ordering::SeqCst) != 0 {
            break 'name_getter;
        }
        let Ok(addr) = game_rva_for_hook(PLAYER_GAME_DATA_NAME_GETTER_RVA as u32) else {
            append_autoload_debug(format_args!(
                "stats-text: REFUSED player-name getter -- rva 0x{PLAYER_GAME_DATA_NAME_GETTER_RVA:x} has no verified mapping for the running build; the other three rows are unaffected"
            ));
            break 'name_getter;
        };
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                player_game_data_name_getter_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                PLAYER_GAME_DATA_NAME_GETTER_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "stats-text: queue_enable player-name getter failed: {status:?}"
                    ));
                    break 'name_getter;
                }
                match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        crate::mh::leak_installed_hook(hook);
                        PLAYER_GAME_DATA_NAME_GETTER_INSTALLED.store(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "stats-text: hooked main-player name getter FUN_14025f8e0 0x{addr:x}; raw PGD name overrides word-checked summary name"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "stats-text: player-name getter MH_ApplyQueued failed: {status:?}"
                    )),
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "stats-text: MhHook::new player-name getter failed: {status:?}"
            )),
        }
    }
    'row_populate: {
        if PROFILE_ROW_POPULATE_INSTALLED.load(Ordering::SeqCst) != 0 {
            break 'row_populate;
        }
        let Ok(addr) = game_rva_for_hook(PROFILE_ROW_POPULATE_RVA as u32) else {
            append_autoload_debug(format_args!(
                "stats-text: REFUSED row-populate -- rva 0x{PROFILE_ROW_POPULATE_RVA:x} has no verified mapping for the running build; the other three rows are unaffected"
            ));
            break 'row_populate;
        };
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                profile_row_populate_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                PROFILE_ROW_POPULATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "stats-text: queue_enable row-populate failed: {status:?}"
                    ));
                    break 'row_populate;
                }
                match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        crate::mh::leak_installed_hook(hook);
                        PROFILE_ROW_POPULATE_INSTALLED.store(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "stats-text: hooked ProfileSelect row-populate FUN_1408758d0 0x{addr:x}; per-slot attributes push before each row's native populate"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "stats-text: row-populate MH_ApplyQueued failed: {status:?}"
                    )),
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "stats-text: MhHook::new row-populate failed: {status:?}"
            )),
        }
    }
    // The row-model builder, hooked separately from the populate above because it is the only place
    // a slot's ProfileSummary record is still a record: it reads `record[0x34]` and the filler turns
    // that into the row's `Location` string. A save whose summary table was copied in from another
    // file needs its place name corrected here or not at all.
    'row_model_build: {
        if PROFILE_ROW_MODEL_BUILD_INSTALLED.load(Ordering::SeqCst) != 0 {
            break 'row_model_build;
        }
        let Ok(addr) = game_rva_for_hook(PROFILE_ROW_MODEL_BUILD_RVA as u32) else {
            append_autoload_debug(format_args!(
                "stats-text: REFUSED row-model-build -- rva 0x{PROFILE_ROW_MODEL_BUILD_RVA:x} has no verified mapping for the running build; the other three rows are unaffected"
            ));
            break 'row_model_build;
        };
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                profile_row_model_build_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                PROFILE_ROW_MODEL_BUILD_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "stats-text: queue_enable row-model-build failed: {status:?}"
                    ));
                    break 'row_model_build;
                }
                match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        crate::mh::leak_installed_hook(hook);
                        PROFILE_ROW_MODEL_BUILD_INSTALLED.store(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "stats-text: hooked ProfileSelect row-model builder FUN_1408752c0 0x{addr:x}; a slot whose summary record is another character's is lent the PlaceName this save evidences for its body's map"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "stats-text: row-model-build MH_ApplyQueued failed: {status:?}"
                    )),
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "stats-text: MhHook::new row-model-build failed: {status:?}"
            )),
        }
    }
    'current_row: {
        if PROFILE_CURRENT_ROW_POPULATE_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET {
            break 'current_row;
        }
        let Ok(addr) = game_rva_for_hook(PROFILE_CURRENT_ROW_POPULATE_RVA as u32) else {
            append_autoload_debug(format_args!(
                "stats-text: REFUSED title-load row-populate -- rva 0x{PROFILE_CURRENT_ROW_POPULATE_RVA:x} has no verified mapping for the running build; the other three rows are unaffected"
            ));
            break 'current_row;
        };
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                profile_current_row_populate_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                PROFILE_CURRENT_ROW_POPULATE_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "stats-text: queue_enable title-load row-populate failed: {status:?}"
                    ));
                    break 'current_row;
                }
                match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        crate::mh::leak_installed_hook(hook);
                        append_autoload_debug(format_args!(
                            "stats-text: hooked title-load current row-populate FUN_140951220 0x{addr:x}; pushes ErCharStats after native current-row populate"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "stats-text: title-load row-populate MH_ApplyQueued failed: {status:?}"
                    )),
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "stats-text: MhHook::new title-load row-populate failed: {status:?}"
            )),
        }
    }
}

#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) unsafe extern "system" fn title_gfx_value_set_visible_hook(
    value: usize,
    visible: u8,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_GFX_VALUE_SET_VISIBLE_ORIG.load(Ordering::SeqCst);
    if orig == null || orig == HOOK_ORIGINAL_UNSET {
        return value;
    }
    let single_target = TITLE_PRESS_START_GFX_VALUE.load(Ordering::SeqCst);
    let in_text_hide_set = TITLE_TEXT_GFX_VALUES.iter().any(|slot| {
        let target = slot.load(Ordering::SeqCst);
        target != null && target != 0 && value == target
    });
    let caller_rva = trace_first_game_caller_rva();
    // The call site is resolved for the running build, not compared against the raw 1.16.2
    // `0x744e02`. Reached raw, this comparison never matched on 1.17 and the title FadeIn
    // suppression was inert with nothing in any log to say so -- no hook to refuse, no address to
    // resolve, so no refusal line either.
    let title_fadein_call_site = title_gfx_visible_title_fadein_caller_rva();
    let title_fadein_visible_ordinal = if title_fadein_call_site == Some(caller_rva) && visible != 0
    {
        TITLE_GFX_VISIBLE_TITLE_FADEIN_SEEN.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        0
    };
    let force_title_fadein_visible =
        title_fadein_visible_ordinal == TITLE_05_000_FADEIN_FLASH_VISIBLE_ORDINAL;
    // All three force the surface to 0, and all three are cover-window behaviour (2026-09-04).
    // `PressStart`, the title text set, and the FadeIn-flash ordinal exist to keep the vanilla title
    // from showing through the product cover. With the cover stopped there is nothing in front of
    // the title, so forcing these to 0 does not hide a seam -- it just deletes the title. Scoping
    // them to the cover window is what keeps a failed load recoverable instead of a black screen;
    // `title_visual_suppression_active` carries the measurement and re-arms on the next cover.
    let forced = er_telemetry_core::counters::title_visual_suppression_active()
        && ((single_target != null && single_target != 0 && value == single_target)
            || in_text_hide_set
            || force_title_fadein_visible);
    let forced_visible = if forced {
        TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE.store(value, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED.store(visible as usize, Ordering::SeqCst);
        0
    } else {
        visible
    };
    if title_fadein_visible_ordinal != 0 && title_fadein_visible_ordinal <= 5 {
        append_autoload_debug(format_args!(
            "gfx-visible-log: value=0x{value:x} requested_visible={visible} forced_visible={forced_visible} forced={forced} forced_title_fadein={force_title_fadein_visible} title_fadein_ordinal={title_fadein_visible_ordinal} caller_rva=0x{caller_rva:x}"
        ));
    }
    let f: unsafe extern "system" fn(usize, u8) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(value, forced_visible) }
}

#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) fn install_title_gfx_value_set_visible_hook() {
    if TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: GFx visibility MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva_for_hook(TITLE_GFX_VALUE_SET_VISIBLE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve GFx visibility setter rva 0x{TITLE_GFX_VALUE_SET_VISIBLE_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_gfx_value_set_visible_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_GFX_VALUE_SET_VISIBLE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable GFx visibility setter failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        // Report the call site this build will actually compare against. Printing
                        // the 1.16.2 constant made the line read as armed on builds where the
                        // comparison could never match.
                        "title-cover-part-a: hooked GFx visibility setter 0x{addr:x}; forcing 05_000_Title FadeIn flash ordinal {TITLE_05_000_FADEIN_FLASH_VISIBLE_ORDINAL} at rva {} false",
                        match title_gfx_visible_title_fadein_caller_rva() {
                            Some(rva) => format!("0x{rva:x}"),
                            None =>
                                "UNRESOLVED on this build -- the FadeIn flash will not be suppressed"
                                    .to_owned(),
                        }
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: GFx visibility MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new GFx visibility setter failed: {status:?}"
        )),
    }
}

#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) fn install_title_logo_force_hidden_hooks() {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: logo-force MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    if TITLE_LOGO_SET_VISIBLE_INSTALLED.load(Ordering::SeqCst) == 0 {
        match game_rva(TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA as u32) {
            Ok(addr) => match unsafe {
                MhHook::new(
                    addr as *mut c_void,
                    title_logo_set_visible_force_hidden_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    TITLE_LOGO_SET_VISIBLE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: queue_enable logo SetVisible failed: {status:?}"
                        ));
                    } else if unsafe { MH_ApplyQueued() } == MH_STATUS::MH_OK {
                        crate::mh::leak_installed_hook(hook);
                        TITLE_LOGO_SET_VISIBLE_INSTALLED.store(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: hooked {TITLE_LOGO_BACK_VIEW_PARTS_NAME} SetVisible 0x{addr:x}; forcing visible=false"
                        ));
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "title-cover-part-a: MhHook::new logo SetVisible failed: {status:?}"
                )),
            },
            Err(_) => append_autoload_debug(format_args!(
                "title-cover-part-a: failed to resolve logo SetVisible rva 0x{TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA:x}"
            )),
        }
    }
    if TITLE_LOGO_CTOR_INSTALLED.load(Ordering::SeqCst) == 0 {
        match game_rva(TITLE_LOGO_BACK_VIEW_PARTS_CTOR_RVA as u32) {
            Ok(addr) => match unsafe {
                MhHook::new(
                    addr as *mut c_void,
                    title_logo_ctor_force_hidden_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    TITLE_LOGO_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: queue_enable logo ctor failed: {status:?}"
                        ));
                    } else if unsafe { MH_ApplyQueued() } == MH_STATUS::MH_OK {
                        crate::mh::leak_installed_hook(hook);
                        TITLE_LOGO_CTOR_INSTALLED.store(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: hooked {TITLE_LOGO_BACK_VIEW_PARTS_NAME} ctor 0x{addr:x}; hiding immediately after construction"
                        ));
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "title-cover-part-a: MhHook::new logo ctor failed: {status:?}"
                )),
            },
            Err(_) => append_autoload_debug(format_args!(
                "title-cover-part-a: failed to resolve logo ctor rva 0x{TITLE_LOGO_BACK_VIEW_PARTS_CTOR_RVA:x}"
            )),
        }
    }
}

#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) fn install_title_logo_start_login_hide_hook() {
    if TITLE_TOP_START_LOGIN_HIDE_INSTALLED.load(Ordering::SeqCst)
        != TITLE_TOP_START_LOGIN_HIDE_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: start-login MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(start_login_addr) = game_rva_for_hook(TITLE_TOP_START_LOGIN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve TitleTopDialog start-login rva 0x{TITLE_TOP_START_LOGIN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            start_login_addr as *mut c_void,
            title_top_start_login_hide_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_TOP_START_LOGIN_HIDE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable start-login hide failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    TITLE_TOP_START_LOGIN_HIDE_INSTALLED
                        .store(TITLE_TOP_START_LOGIN_HIDE_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked TitleTopDialog start-login 0x{start_login_addr:x}; will hide {TITLE_LOGO_BACK_VIEW_PARTS_NAME}/{TITLE_LOGO_RESOURCE_NAME} after native SetVisible(1)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: start-login MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new start-login hide failed: {status:?}"
        )),
    }
}

/// Install the Part-A title visual suppression hook once. It must run at process attach before
/// STEP_BeginTitle; installing from the recurring game task can be too late for the first title build.
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) fn install_title_pab_information_visual_hook() {
    if TITLE_PAB_INFORMATION_VISUAL_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: PAB/TitleInformation MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva_for_hook(TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve PAB/TitleInformation wrapper rva 0x{TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_pab_information_visual_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_PAB_INFORMATION_VISUAL_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable PAB/TitleInformation wrapper failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    TITLE_PAB_INFORMATION_VISUAL_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked PAB/TitleInformation wrapper 0x{addr:x}; native {TITLE_PAB_INFORMATION_VISUAL_NAME} preserved and covered"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: PAB/TitleInformation MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new PAB/TitleInformation wrapper failed: {status:?}"
        )),
    }
}

#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) fn install_title_native_menu_visual_suppression_hook() {
    if TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED.load(Ordering::SeqCst)
        != TITLE_NATIVE_MENU_VISUAL_SUPPRESS_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(begin_title_addr) = game_rva_for_hook(TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA as u32)
    else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve BeginTitle visual wrapper rva 0x{TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            begin_title_addr as *mut c_void,
            title_native_menu_visual_begin_title_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_NATIVE_MENU_VISUAL_SUPPRESS_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable BeginTitle wrapper failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED.store(
                        TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked BeginTitle visual wrapper 0x{begin_title_addr:x}; native {TITLE_NATIVE_MENU_VISUAL_NAME} MenuWindowJob will be replaced by {TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME}, STEP_Wait/CSMenuMan+0x21 untouched"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new BeginTitle wrapper failed: {status:?}"
        )),
    }
}

#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) fn install_title_native_menu_visual_render_suppression_hook() {
    if TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED.load(Ordering::SeqCst)
        != TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: render MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(fadein_addr) = game_rva_for_hook(TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RVA as u32)
    else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve MenuWindowJob FadeIn helper rva 0x{TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            fadein_addr as *mut c_void,
            title_native_menu_visual_window_fadein_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable FadeIn helper failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED.store(
                        TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked MenuWindowJob FadeIn helper 0x{fadein_addr:x}; preserved native {TITLE_NATIVE_MENU_VISUAL_NAME} will clear visible flags mask 0x{TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK:x} from CSMenuMan+0x90 when Run returns at rva {}",
                        match title_native_menu_visual_window_fadein_run_caller_rva() {
                            Some(rva) => format!("0x{rva:x}"),
                            None => "UNRESOLVED on this build".to_owned(),
                        }
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: render MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new FadeIn helper failed: {status:?}"
        )),
    }
}

pub(crate) fn system_quit_list_slot_addr(list: usize, slot: usize) -> usize {
    list.wrapping_add((0usize.wrapping_sub(list)) & 7)
        .wrapping_add(slot * std::mem::size_of::<usize>())
}

// The System-window hide/restore now lives in `er-quit-menu-core`, so a standalone quit-menu
// shell can put the pause menu behind its ProfileSelect overlay and take it back afterwards. These
// wrappers keep this crate's call sites unchanged and supply the steps only a host with a character
// switch behind it can perform.
use er_quit_menu_core::system_windows::{self, SystemWindowHooks};

pub(crate) use er_quit_menu_core::system_windows::{
    hide_real_system_windows as system_quit_hide_real_system_windows,
    note_profile_select_finalized as system_quit_note_profile_select_finalized,
};

/// Whether a character switch is mid-flight, which is what makes a restore the wrong move.
fn switch_in_flight() -> bool {
    SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
}

/// The switch's own restore-time work. Answers whether the return-title chain was submitted.
///
/// # Safety
///
/// Menu-pump context, the same one the restore runs in.
unsafe fn switch_restore(base: usize, source: &str) -> bool {
    // Keep the native quit-save unblocked every frame the switch is active. On a 2nd in-process
    // switch a stale `CSMenuMan->disableSaveMenu` aborts the quit-save so `bc4` freezes at 1 and the
    // world never tears down; clearing it once at the request can be re-set before the save
    // orchestrator polls, so it is cleared here too (a no-op once it is 0).
    unsafe { system_quit_clear_disable_save_menu(base, source) };
    // Diagnostic: name which of the save orchestrator's three gates is freezing `bc4` at 1.
    unsafe { system_quit_log_save_gates(base, source) };
    let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
    let submitted =
        unsafe { system_quit_submit_direct_return_title_chain(base, system_dialog, source) };
    SYSTEM_QUIT_SKIP_RESTORE_AFTER_QUICKLOAD_COUNT.fetch_add(1, Ordering::SeqCst);
    if submitted {
        SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
    }
    submitted
}

/// The product's full set: it owns the picker, the editor field targets and the switch.
fn product_hooks() -> SystemWindowHooks {
    SystemWindowHooks {
        save_picker_reset: Some(save_picker_reset),
        forget_profile_editor_field_targets: Some(|source| {
            super::forget_profile_editor_field_targets(source)
        }),
        switch_in_flight: Some(switch_in_flight),
        switch_restore: Some(switch_restore),
        save_swap_restore_profile_summary: Some(system_quit_save_swap_restore_profile_summary),
    }
}

/// # Safety
///
/// Menu-pump context.
pub(crate) unsafe fn system_quit_reset_profile_select_state(source: &str) {
    unsafe { system_windows::reset_profile_select_state(source, &product_hooks()) };
}

/// # Safety
///
/// Menu-pump context.
pub(crate) unsafe fn system_quit_restore_real_system_windows(base: usize, source: &str) {
    unsafe { system_windows::restore_real_system_windows(base, source, &product_hooks()) };
}

pub(crate) fn system_quit_read_wide_resource_name(ptr: usize) -> String {
    const MAX_UNITS: usize = 64;
    if ptr < 0x10000 {
        return String::new();
    }
    let mut units = Vec::new();
    for idx in 0..MAX_UNITS {
        let unit = unsafe { safe_read_u16(ptr + idx * 2) }.unwrap_or(0);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

/// Clear a stale `CSMenuMan->disableSaveMenu` (BOOL @ +0x13c) so the native quit-save can run during a
/// System->Quit switch. RE of the 1.16.1 dump (2026-07-16, persistent Ghidra project) proved the quit-save
/// (GameMan `bc4` 1->2 pump `FUN_14067b840`/`FUN_14067ba30`, and `ShouldSave`) ABORTS -- clearing
/// `saveRequested` -- the instant this byte is non-zero (`CanShowSaveMenu` returns it directly). On a 2nd
/// in-process switch it is left set from the prior switch's menu flow, so the quit-save never runs, `bc4`
/// freezes at 1, and the world never tears down (the switch-2 soft-lock). Switch 1 has it 0. Called every
/// frame the switch is active so it holds until the game's save orchestrator polls `saveRequested`, plus
/// once at the return-title request for pre-clear telemetry. Returns the pre-clear value (-1 if CSMenuMan
/// unavailable). Only ever called from switch-active paths, and a no-op when already 0, so normal-gameplay
/// save-disable behaviour is untouched.
pub(crate) unsafe fn system_quit_clear_disable_save_menu(base: usize, source: &str) -> i32 {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let cs_menu_man = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        ))
    }
    .unwrap_or(NULL);
    if cs_menu_man < HEAP_LO {
        return -1;
    }
    let dsm = (cs_menu_man + CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET) as *mut u8;
    let prev = unsafe { dsm.read_volatile() };
    if prev != 0 {
        unsafe { dsm.write_volatile(0) };
        let n = SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 5 || n.is_multiple_of(120) {
            append_autoload_debug(format_args!(
                "system-quit-quickload: cleared stale CSMenuMan->disableSaveMenu (was {prev}) #{n} source={source} -- native quit-save was gated OFF (bc4 freezes at 1, world never tears down); now unblocked so bc4 pumps 1->2->3"
            ));
        }
    }
    prev as i32
}

/// Drive the return-title predicate `GameMan+0xbc4` straight to ready(3) right after the native request
/// set it to 1. Saving is disabled by design (the in-game "Save Game" button is the only save writer),
/// so the game's quit-save -- which is the only thing that natively pumps bc4 1->2->3 (dump
/// FUN_14067b840: the bc4 1->2 advance is welded to a successful disk write `cVar4 != 0`) -- will never
/// run. Forcing bc4=ready here is the single deterministic write that completes the switch without a
/// save: (a) it satisfies the final-functor gate in `product_core_autoload_tick` (which needs bc4==ready
/// to submit the return-title job that sets rt5d and tears the old world down), and (b) it SUPPRESSES the
/// quit-save itself -- the orchestrator's `ShouldSave` and `FUN_140679460` both require bc4 != 3, so no
/// disk write is attempted and no "failed to save" popup can appear. Returns the pre-force bc4 value
/// (-1 if GameMan unavailable). The incoming world's STEP_MoveMap(18) finalize gate (blocked while
/// bc4 != 0) is released later by the deterministic streamed-and-parked bc4->0 clear on the game task.
pub(crate) unsafe fn system_quit_force_return_title_bc4_ready(base: usize, source: &str) -> i32 {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    const GAME_MAN_SINGLETON_RVA: usize = er_game_base::rva::GAME_MAN_SINGLETON_RVA;
    let gm = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_MAN_SINGLETON_RVA,
            "GAME_MAN_SINGLETON_RVA",
        ))
    }
    .unwrap_or(NULL);
    if gm < HEAP_LO {
        return -1;
    }
    let bc4p = (gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) as *mut i32;
    let prev = unsafe { bc4p.read_volatile() };
    unsafe { bc4p.write_volatile(GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY as i32) };
    let n = SYSTEM_QUIT_BC4_FORCE_READY_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "system-quit-quickload: forced return-title bc4 {prev}->READY(3) #{n} source={source} -- save disabled by design, so the game never pumps bc4 via the quit-save; this both fires the final functor and suppresses the quit-save (no disk write)"
    ));
    prev
}

/// Diagnostic (switch-2 save-freeze): the quit-save orchestrator `FUN_140afb970` (RE of the 1.16.1 dump)
/// gates the save on three conditions, any of which blocks it and freezes `bc4` at 1: (a) `BOOL_143d856a0`
/// -- the load-active / title-accept latch, RVA `0x3d856a0` -- must be 0 (it returns early otherwise); (b)
/// `GameMan->save_state` (== our b80 offset) must be 0 (`FUN_14067a170`); (c) the menu gate `FUN_14080d660`:
/// `*(CSMenuMan+0x80)->0x290` (byte) == 0 and `->0x298` (qword) == 0. `save_state` is already 0 at the
/// freeze, so this logs all three per-frame during the switch to name the actual blocker. Read-only.
pub(crate) unsafe fn system_quit_log_save_gates(base: usize, source: &str) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    // The engine SHUTDOWN/CLEANUP flag, not a "force latch" -- read-only here. See
    // er_title_flow::TITLE_ACCEPT_LATCH_RVA for the evidence.
    const FORCE_LATCH_RVA: usize = TITLE_ACCEPT_LATCH_RVA;
    const GAME_MAN_SINGLETON_RVA: usize = er_game_base::rva::GAME_MAN_SINGLETON_RVA;
    let n = SYSTEM_QUIT_SAVE_GATE_DIAG_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    if !(n <= 8 || n.is_multiple_of(240)) {
        return;
    }
    let force = unsafe {
        safe_read_u8(er_game_base::mem::game_data_addr(
            base,
            FORCE_LATCH_RVA,
            "FORCE_LATCH_RVA",
        ))
    }
    .unwrap_or(0xff);
    let gm = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_MAN_SINGLETON_RVA,
            "GAME_MAN_SINGLETON_RVA",
        ))
    }
    .unwrap_or(NULL);
    let (save_state, bc4) = if gm >= HEAP_LO {
        (
            unsafe { safe_read_i32(gm + GAME_MAN_SAVE_STATE_B80_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_i32(gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) }
                .unwrap_or(-1),
        )
    } else {
        (-1, -1)
    };
    let csm = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        ))
    }
    .unwrap_or(NULL);
    let sub = if csm >= HEAP_LO {
        unsafe { safe_read_usize(csm + 0x80) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let (m290, m298) = if sub >= HEAP_LO {
        (
            unsafe { safe_read_u8(sub + 0x290) }.unwrap_or(0xff),
            unsafe { safe_read_usize(sub + 0x298) }.unwrap_or(usize::MAX),
        )
    } else {
        (0xff, usize::MAX)
    };
    let menu_gate_ok = m290 == 0 && m298 == 0;
    let blocker = if force != 0 {
        "FORCE_LATCH(0x143d856a0!=0)"
    } else if save_state != 0 {
        "save_state!=0"
    } else if !menu_gate_ok {
        "MENU_GATE(CSMenuMan+0x80.290/298)"
    } else {
        "NONE(save should run)"
    };
    append_autoload_debug(format_args!(
        "save-gate-diag #{n} source={source}: force=0x{force:x} save_state={save_state} bc4={bc4} menu290=0x{m290:x} menu298=0x{m298:x} menu_gate_ok={menu_gate_ok} -> quit-save blocked by {blocker}"
    ));
}

pub(crate) unsafe fn system_quit_submit_direct_return_title_chain(
    base: usize,
    system_dialog: usize,
    source: &str,
) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) != 0 {
        return true;
    }
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if !(SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        return true;
    }
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- system_dialog=0x{system_dialog:x} not heap-like"
        ));
        return false;
    }
    let queue = system_dialog + 0x10;
    let list = system_dialog + 0x50;
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_DIALOG.store(system_dialog, Ordering::SeqCst);
    let Ok(ready_addr) = game_rva(MENU_JOB_QUEUE_READY_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- queue-ready rva 0x{MENU_JOB_QUEUE_READY_RVA:x} unresolved"
        ));
        return false;
    };
    let ready_fn: unsafe extern "system" fn(usize) -> u8 =
        unsafe { std::mem::transmute(ready_addr) };
    let queue_ready = unsafe { ready_fn(queue) } != 0;
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_QUEUE_READY
        .store(queue_ready as usize, Ordering::SeqCst);
    if !queue_ready {
        let waits = SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_READY_BLOCK_COUNT
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if waits <= 3 || waits.is_multiple_of(60) {
            let head = unsafe { safe_read_usize(queue) }.unwrap_or(NULL);
            let pending6 = unsafe { safe_read_usize(queue + 0x30) }.unwrap_or(NULL);
            append_autoload_debug(format_args!(
                "system-quit-quickload: direct return-title chain WAIT source={source} waits={waits} queue not ready dialog=0x{system_dialog:x} queue=0x{queue:x} head=0x{head:x} field6=0x{pending6:x}"
            ));
        }
        return false;
    }
    // Fire the native return-title request (FUN_14067a490, live 0x67a3a0) -- the missing piece. It sets
    // GameMan.saveRequested = true and GameMan+0xbc4 = 1 (== GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY).
    // Without it, bc4 stays 0, so (a) the game never recognizes a return-to-title is pending and never
    // saves+tears down the world, and (b) our final functor (title.rs, gated on bc4==ready) never fires,
    // leaving the submitted chain job orphaned in a queue that stops being pumped once the menus close.
    // Observed 2026-07-01: OK -> menus closed but still in-world, same char, functor_call_count=0,
    // bc4=0, native_quit_action_count=0. The native Quit-Game does this request and the build+submit
    // below; we were doing only the build+submit. It is a plain GameMan field write (+ FUN_14080dd00),
    // safe to call from this menu-pump-owned path. Fire once. See bd
    // system-quit-loadjob-success-commits-phantom-load-2026-07-01.
    if SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.load(Ordering::SeqCst) == 0 {
        match game_rva(SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA) {
            Ok(req_addr) => {
                let request_fn: unsafe extern "system" fn() =
                    unsafe { std::mem::transmute(req_addr) };
                unsafe { request_fn() };
                SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
                // The request just set saveRequested + bc4=1. Saving is disabled by design (only the in-game
                // "Save Game" button writes), so the game's quit-save -- the only native pump of bc4 1->2->3 --
                // must not run. Drive bc4 straight to ready(3) ourselves: this fires the final functor (which
                // needs bc4==ready) and suppresses the quit-save (ShouldSave/FUN_140679460 require bc4 != 3), so
                // the switch completes with no disk write and no "failed to save" popup. Deterministic, keyed on
                // the request we just fired -- not a frame counter. See system_quit_force_return_title_bc4_ready.
                let bc4_prev = unsafe { system_quit_force_return_title_bc4_ready(base, source) };
                append_autoload_debug(format_args!(
                    "system-quit-quickload: native return-title REQUEST fired 0x{req_addr:x} source={source} -- set saveRequested + bc4=1, then forced bc4 {bc4_prev}->READY(3) (save disabled by design; functor can fire, quit-save suppressed)"
                ));
            }
            Err(_) => append_autoload_debug(format_args!(
                "system-quit-quickload: return-title request rva 0x{SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA:x} unresolved source={source}"
            )),
        }
    }
    // No native confirm chain is submitted (P0 fix, run br-20260831-160354-2513).
    // `SYSTEM_QUIT_RETURN_TITLE_CHAIN_BUILDER_RVA` (1.16.2 0x79d700 == 1.17 0x79e580) builds a
    // FixOrderJobSequence of five jobs whose head is a MessageBox job (FUN_1407b73d0) carrying
    // `GetGR_Dialogues(110000)` -- engus "Save the game and return to title menu?", anchor
    // L"\u{6c7a}\u{5b9a}" -- the vanilla Quit-Game confirm. Submitting it is the sole reason a
    // `CS::MessageBoxDialog` was ever built on the switch path (`msgbox-skip #0
    // scope=switch-active`; game callers 0x1407b1347 = the dialog factory FUN_1407b1270,
    // 0x1407ae13c = CS::MenuWindowJob::Run, 0x1407ab13b = FixOrderJobSequence::Run).
    // The box's only semantic side effect is the user's yes/no gate on advancing that sequence to
    // its final job, FUN_14079f690 == `SYSTEM_QUIT_RETURN_TITLE_FINAL_JOB_BUILDER_RVA` -- which we
    // already build and submit ourselves, without UI, from the bc4==ready path (that path also owns
    // `system_quit_save_swap_recommit_after_return_title_save`, so it must stay where it is).
    // With the head suppressed its factory returns null, and `MenuWindowJob::Run`'s
    // `owningMenuWindow == 0` path sets MenuJobResult Failed -- terminal after one Run (hence
    // exactly one msgbox-skip, never a #1), so the sequence aborts and jobs 2..5 never execute. The
    // submit was pure overhead whose one observable effect was a MessageBoxDialog build, plus a job
    // that held the queue not-ready and delayed the real final functor. Bump the counter that path
    // gates on and submit nothing.
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-quickload: direct return-title chain ARMED source={source} dialog=0x{system_dialog:x} queue=0x{queue:x} list=0x{list:x} -- native confirm chain 0x{SYSTEM_QUIT_RETURN_TITLE_CHAIN_BUILDER_RVA:x} deliberately NOT submitted (its head is the GR_Dialogues(110000) return-to-title MessageBox); the final functor fires from the bc4==READY path"
    ));
    true
}

pub(crate) unsafe fn system_quit_profile_select_top_menu_tick() {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let hidden = SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if !hidden {
        return;
    }
    if profile == 0 {
        // ProfileSelect has closed. Do not submit the return-title chain from this game-task tick:
        // that runs concurrently with the game's own menu/Scaleform pump and corrupts it (observed:
        // non-deterministic execute-fault jumping into Scaleform string data). The close is done in
        // menu-pump ownership by the native confirm transition (dialog+0x1e8=Success pops the
        // ProfileSelect window job) and the return-title submit is done in menu-pump ownership from
        // the MenuWindowJob::Run hook. See bd system-quit-return-title-scaleform-race-2026-07-01.
        // Save-picker navigation closes have a resubmit queued (menu-pump owned): the staged
        // rows / applied preview must survive until the window reopens -- do not restore here.
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) == SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
            && !save_picker_resubmit_pending()
        {
            if let Ok(base) = game_module_base() {
                unsafe {
                    system_quit_restore_real_system_windows(
                        base,
                        "restore-real-profile-closed-without-load",
                    )
                };
            } else {
                unsafe {
                    system_quit_save_swap_restore_profile_summary(
                        "profile-select-closed-without-load-no-base",
                    )
                };
                unsafe {
                    system_quit_reset_profile_select_state(
                        "profile-select-closed-without-load-no-base",
                    )
                };
            }
        }
        return;
    }
    if let Ok(base) = game_module_base() {
        unsafe { system_quit_save_swap_poll_preview(base) };
    }
    let list = SYSTEM_QUIT_TOP_HIDE_LIST.load(Ordering::SeqCst);
    if list == 0 {
        return;
    }
    let count = unsafe { safe_read_usize(list + 0x48) }.unwrap_or(0);
    let still_present = (0..count.min(8)).any(|idx| {
        unsafe { safe_read_usize(system_quit_list_slot_addr(list, idx)) }.unwrap_or(NULL) == profile
    });
    if still_present {
        return;
    }
    if save_picker_resubmit_pending() {
        // Mid picker navigation: the window left the list on its way to a menu-pump-owned
        // resubmit; restoring from this game-task tick would clobber the staged rows.
        return;
    }
    if let Ok(base) = game_module_base() {
        unsafe { system_quit_restore_real_system_windows(base, "restore-real-profile-left-list") };
    } else {
        unsafe { system_quit_reset_profile_select_state("restore-real-profile-left-list-no-base") };
    }
}

/// Result of resolving one named OptionSetting child and reading its DisplayInfo.Visible.
pub(crate) struct OptionSettingPaneSample {
    /// `assignComponentWithName` returned a live out proxy (not 0 / not the null sentinel).
    resolved: bool,
    /// The resolved child's CSScaleformValue is a live DisplayObject (`(dataType & MASK) == VALUE`).
    #[allow(dead_code)]
    // Retained: Populated for the gate diagnosis this sample exists to record; nothing reads it back yet.
    is_display: bool,
    /// DisplayInfo.Visible byte was nonzero after the `GetDisplayInfo` vcall.
    visible: bool,
    /// Raw dataType (for gate diagnosis when `is_display` is false).
    datatype: i32,
}

/// Read-ONLY: resolve one named child of the OptionSetting root proxy and read its
/// DisplayInfo.Visible. Mirrors `push_stats_text_on_row`'s resolve/guard/release exactly -- native
/// `assignComponentWithName` into a zeroed out proxy, the 7e7 game-image guard on the vptr chain
/// before any virtual dispatch, and `~CSScaleformValue` on the out proxy's embedded value (+0x28).
/// Nothing is mutated; the `GetDisplayInfo` vcall only fills the caller's stack buffer. dtor is run
/// exactly once for every resolved out proxy (never for an unresolved name).
pub(crate) unsafe fn resolve_optionsetting_pane(
    base: usize,
    assign: unsafe extern "system" fn(usize, usize, usize) -> usize,
    dtor: unsafe extern "system" fn(usize),
    root_proxy: usize,
    name: &str,
) -> OptionSettingPaneSample {
    debug_assert!(name.ends_with('\0'), "pane name must be NUL-terminated");
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // The binder fully constructs the out proxy before reading it; a zeroed 0x80-byte buffer mirrors
    // the native uninitialized stack slot. Names carry no '%', safe as the binder's printf format.
    let mut out_buf = [0u8; SCENE_OBJ_PROXY_STACK_BYTES];
    let out = unsafe {
        assign(
            root_proxy,
            out_buf.as_mut_ptr() as usize,
            name.as_ptr() as usize,
        )
    };
    if out == 0 || out == null {
        return OptionSettingPaneSample {
            resolved: false,
            is_display: false,
            visible: false,
            datatype: 0,
        };
    }
    let cs_value = out + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET;
    let (is_display, visible, datatype) = unsafe { read_scaleform_pane_visible(base, cs_value) };
    unsafe { dtor(cs_value) };
    OptionSettingPaneSample {
        resolved: true,
        is_display,
        visible,
        datatype,
    }
}

/// Read `DisplayInfo.Visible` from a `CSScaleformValue` at `cs_value`. Returns
/// `(is_display, visible, datatype)`. Read-ONLY: the `GetDisplayInfo` vcall only fills a local buffer;
/// this does not release the value (the caller owns lifetime -- an assign'd out proxy is dtor'd by the
/// caller; an embedded proxy has nothing to release). 7e7 guard on the vptr chain before any dispatch:
/// validate the vtable (`*objectInterface`) and the resolved fn are game-image-live (not the heap
/// objectInterface instance itself). `safe_read` of `*objectInterface` fails closed if unmapped.
pub(crate) unsafe fn read_scaleform_pane_visible(
    base: usize,
    cs_value: usize,
) -> (bool, bool, i32) {
    let object_interface =
        unsafe { safe_read_usize(cs_value + CSSCALEFORMVALUE_OBJECT_INTERFACE_OFFSET) }
            .unwrap_or(0);
    let datatype =
        unsafe { safe_read_i32(cs_value + CSSCALEFORMVALUE_DATATYPE_OFFSET) }.unwrap_or(0);
    let value_handle =
        unsafe { safe_read_usize(cs_value + CSSCALEFORMVALUE_HANDLE_OFFSET) }.unwrap_or(0);
    let is_display =
        (datatype & CSSCALEFORMVALUE_DISPLAY_TYPE_MASK) == CSSCALEFORMVALUE_DISPLAY_TYPE_VALUE;
    if !is_display {
        return (false, false, datatype);
    }
    let vfptr = unsafe { safe_read_usize(object_interface) }.unwrap_or(0);
    let getfn = if vfptr != 0 {
        unsafe { safe_read_usize(vfptr + CSSCALEFORMVALUE_GET_DISPLAY_INFO_VTABLE_SLOT) }
            .unwrap_or(0)
    } else {
        0
    };
    let guarded = object_interface != 0
        && vfptr != 0
        && vtable_in_game_image(vfptr, base)
        && getfn != 0
        && vtable_in_game_image(getfn, base);
    if !guarded {
        OPTIONSETTING_PANE_GUARD_SKIPS.fetch_add(1, Ordering::SeqCst);
        return (true, false, datatype);
    }
    let getfn: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(getfn) };
    let mut info = [0u8; OPTIONSETTING_DISPLAY_INFO_BYTES];
    unsafe { getfn(object_interface, value_handle, info.as_mut_ptr() as usize) };
    (
        true,
        info[OPTIONSETTING_DISPLAY_INFO_VISIBLE_OFFSET] != 0,
        datatype,
    )
}

pub(crate) fn wide_ptr_starts_with_ascii(ptr: usize, ascii: &[u8]) -> bool {
    if ptr == TITLE_OWNER_SCAN_START_ADDRESS || ascii.is_empty() {
        return false;
    }
    for (idx, &want) in ascii.iter().enumerate() {
        let Some(unit) = (unsafe { safe_read_u16(ptr + idx * 2) }) else {
            return false;
        };
        if unit != want as u16 {
            return false;
        }
    }
    true
}

/// Which of the five Quit-tab labels a row carries (0 = none of ours). Telemetry only
/// (`oracle_optionsetting_active_row_quit_label_mask`); the routing identity lives in
/// `system_quit_row_label_at`.
///
/// "Load Character from File" is tested before "Load Character", because the first string starts
/// with the second and a prefix test in the other order would report every file-browse row as the
/// character row. The pre-2026-07-31 labels did not overlap, so this order was arbitrary then.
pub(crate) fn optionsetting_quit_label_kind(label_ptr: usize) -> usize {
    if wide_ptr_starts_with_ascii(label_ptr, b"Save Game") {
        1
    } else if wide_ptr_starts_with_ascii(label_ptr, b"Load Character from File") {
        3
    } else if wide_ptr_starts_with_ascii(label_ptr, b"Load Character") {
        2
    } else if wide_ptr_starts_with_ascii(label_ptr, b"Return to Desktop") {
        4
    } else if wide_ptr_starts_with_ascii(label_ptr, b"Load Build from URL") {
        5
    } else {
        0
    }
}

pub(crate) fn hash_wide_label_ptr(label_ptr: usize) -> usize {
    let mut hash = FNV1A64_OFFSET_BASIS as usize;
    if label_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return hash;
    }
    for idx in 0..48usize {
        let Some(unit) = (unsafe { safe_read_u16(label_ptr + idx * 2) }) else {
            break;
        };
        // Preserve this diagnostic signature's historical non-FNV multiplier exactly. It is not
        // a content fingerprint and therefore is not routed through the canonical FNV round.
        hash ^= unit as usize;
        hash = hash.wrapping_mul(0x1000_0000_01b3usize);
        if unit == 0 {
            break;
        }
    }
    hash
}

pub(crate) unsafe fn sample_optionsetting_active_row_table(
    current_dialog: usize,
    current_tab: usize,
    actively_shown: bool,
) {
    const HEAP_LO: usize = 0x10000;
    const MAX_ROWS: usize = 16;
    pub(crate) use er_telemetry_core::counters::OPTIONSETTING_ROW_LAST_LOG_KEY;
    if !actively_shown || current_dialog < HEAP_LO {
        return;
    }
    let count = unsafe {
        safe_read_usize(current_dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
    }
    .unwrap_or(0)
    .min(MAX_ROWS);
    let properties = current_dialog + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET;
    let aligned_properties = (properties + 0x7) & !0x7;
    // Compare controllers, not the `+0xa8` "action object": that field is only `controller + 0x70`
    // (the controller's own inline std::function storage), so an action comparison is a controller
    // comparison in disguise -- and a captured controller from a dead dialog can be matched by a
    // reused heap address. Requiring the row table's dialog removes that stale-match class; the mask
    // stays purely diagnostic either way.
    let table_dialog = SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst);
    let table_live = table_dialog != 0 && table_dialog == current_dialog;
    let quickload_controller = if table_live {
        SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
    } else {
        0
    };
    let open_profiles_controller = if table_live {
        SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
    } else {
        0
    };
    let build_url_controller = if table_live {
        SYSTEM_QUIT_LOAD_BUILD_URL_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
    } else {
        0
    };
    let generate_link_controller = if table_live {
        SYSTEM_QUIT_GENERATE_BUILD_LINK_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
    } else {
        0
    };
    let native_save_controller = if table_live {
        SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
    } else {
        0
    };
    let mut cloned_mask = 0usize;
    let mut native_save_mask = 0usize;
    let mut quit_label_mask = 0usize;
    let mut action_hash = fnv1a64(b"") as usize;
    let mut label_hash = fnv1a64(b"") as usize;
    for row_idx in 0..count {
        let row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(row_idx);
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
        action_hash = action_hash.rotate_left(5) ^ action.wrapping_mul(0x9e37_79b9_7f4a_7c15usize);
        let label_ptr = unsafe { safe_read_usize(row + 0x8) }.unwrap_or(0);
        let row_label_hash = hash_wide_label_ptr(label_ptr);
        label_hash = label_hash.rotate_left(7) ^ row_label_hash;
        if optionsetting_quit_label_kind(label_ptr) != 0 {
            quit_label_mask |= 1usize << row_idx;
        }
        if controller != 0
            && (controller == quickload_controller
                || controller == open_profiles_controller
                || controller == build_url_controller
                || controller == generate_link_controller)
        {
            cloned_mask |= 1usize << row_idx;
        }
        if controller != 0 && controller == native_save_controller {
            native_save_mask |= 1usize << row_idx;
        }
    }
    OPTIONSETTING_ACTIVE_ROW_SAMPLE_COUNT.fetch_add(1, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_DIALOG.store(current_dialog, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_TAB.store(current_tab, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_COUNT.store(count, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_CLONED_MASK.store(cloned_mask, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_NATIVE_SAVE_MASK.store(native_save_mask, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_ACTION_HASH.store(action_hash, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_LABEL_HASH.store(label_hash, Ordering::SeqCst);
    OPTIONSETTING_ACTIVE_ROW_QUIT_LABEL_MASK.store(quit_label_mask, Ordering::SeqCst);
    if current_tab == 0 && cloned_mask != 0 {
        OPTIONSETTING_GAME_OPTIONS_CLONED_ROW_HITS.fetch_add(1, Ordering::SeqCst);
    }
    if current_tab == 0 && quit_label_mask != 0 {
        OPTIONSETTING_GAME_OPTIONS_QUIT_LABEL_HITS.fetch_add(1, Ordering::SeqCst);
    }
    let log_key = ((current_tab & 0xff) << 56)
        ^ ((count & 0xff) << 48)
        ^ ((cloned_mask & 0xff) << 32)
        ^ ((native_save_mask & 0xff) << 24)
        ^ ((quit_label_mask & 0xff) << 16)
        ^ (action_hash & 0xffff);
    if OPTIONSETTING_ROW_LAST_LOG_KEY.swap(log_key, Ordering::SeqCst) != log_key
        || (current_tab == 0 && (cloned_mask != 0 || quit_label_mask != 0))
    {
        append_autoload_debug(format_args!(
            "optionsetting-rows: active tab={current_tab} dialog=0x{current_dialog:x} count={count} cloned_mask=0x{cloned_mask:x} native_save_mask=0x{native_save_mask:x} quit_label_mask=0x{quit_label_mask:x} action_hash=0x{action_hash:x} label_hash=0x{label_hash:x} table_live={table_live} quickload_controller=0x{quickload_controller:x} open_profiles_controller=0x{open_profiles_controller:x} native_save_controller=0x{native_save_controller:x}"
        ));
    }
}

/// Read-only oracle: on OptionSetting menu re-entry, read whether the option-row pane display
/// objects are actually visible. Detects the "blank Game Options pane" bug (tab strip + footer
/// render, row list is black) with no screenshot. This also owns the active Game Options tab-entry
/// repair: when the visible selected tab is 0, re-assert the cached/native Game Options pane once on
/// entry so stale Quit-tab rows cannot remain cross-populated under the vanilla Game Options tab.
/// Runs on the menu/game thread (the `MenuWindowJob::Run` hook) as required for GFx vcalls.
pub(crate) unsafe fn sample_optionsetting_pane_visibility(base: usize, option_window: usize) {
    pub(crate) use er_telemetry_core::counters::OPTIONSETTING_LAST_ACTIVE_TAB;
    if option_window == 0 || option_window < OPTIONSETTING_WINDOW_MIN_PTR {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Prefer the hooked ORIG trampoline so the resolve is not double-instrumented (as in
    // push_stats_text_on_row); else the game function, resolved for the running build.
    //
    // The fallback arm used to be a bare `base + RVA`. It is reached exactly when the detour is
    // not installed -- which on a moved build is the likeliest state, because an unmapped hook
    // target is refused -- so the one path that runs without the hook was the one path that never
    // asked where the function went. `CS::SceneObjProxy` named-child bind moved on 1.17
    // (0x74a2f0 -> 0x74b140, byte-checked: 1.16.2 @0x74a2f0 and 1.17 @0x74b140 are the same
    // prologue `4c 89 44 24 18 4c 89 4c 24 20 55 53 56 57 41 56`, while 1.17 @0x74a2f0 is
    // mid-instruction), so the fallback pointed into unrelated code. `gated_game_fn` refuses
    // instead, and the pane-visibility oracle simply does not sample.
    let assign_addr = match TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG.load(Ordering::SeqCst) {
        orig if orig != null && orig != HOOK_ORIGINAL_UNSET => orig,
        _ => match crate::experiments::gated_game_fn(
            TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA,
            "TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA",
        ) {
            Some(address) => address,
            None => return,
        },
    };
    let assign: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(assign_addr) };
    let dtor: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                CSSCALEFORMVALUE_DTOR_RVA,
                "CSSCALEFORMVALUE_DTOR_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    let root_proxy = option_window + OPTION_SETTING_ROOT_PROXY_OFFSET;

    // The pane CONTAINER: its resolved-but-not-visible state is the direct blank-pane signature.
    let wl = unsafe {
        resolve_optionsetting_pane(
            base,
            assign,
            dtor,
            root_proxy,
            OPTIONSETTING_WINDOWLIST_NAME,
        )
    };

    // Each option pane -> per-pane resolved/visible bitmasks (bit index = pane order).
    let mut resolved_mask: usize = 0;
    let mut visible_mask: usize = 0;
    for (idx, &name) in OPTIONSETTING_PANE_NAMES.iter().enumerate() {
        let sample = unsafe { resolve_optionsetting_pane(base, assign, dtor, root_proxy, name) };
        if sample.resolved {
            resolved_mask |= 1usize << idx;
        }
        if sample.visible {
            visible_mask |= 1usize << idx;
        }
    }

    let composite = option_window + OPTIONSETTING_COMPOSITE_OFFSET;
    let composite_bound =
        unsafe { safe_read_usize(composite + OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET) }
            .map(|v| v != 0)
            .unwrap_or(false);

    // The real SIGNAL: the game's tab-select (FUN_14093b850) toggles SetVisible on the current tab
    // dialog's embedded proxy at dialog+0x1200 -- Not the named WindowList children (which stay
    // Visible=0 always). current dialog = *(composite+0xb8).
    let current_dialog =
        unsafe { safe_read_usize(composite + OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET) }
            .unwrap_or(0);
    let (cur_is_display, cur_visible, cur_dt) = if current_dialog >= OPTIONSETTING_WINDOW_MIN_PTR {
        unsafe {
            read_scaleform_pane_visible(
                base,
                current_dialog
                    + OPTIONSETTING_DIALOG_PANE_PROXY_OFFSET
                    + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET,
            )
        }
    } else {
        (false, false, 0)
    };

    // "Actively shown" gate: CSMenuMan flag byte bit 0x4 = the window is drawn this frame. The
    // OptionSetting MenuWindowJob::Run also fires during preload/hidden states; without this gate the
    // blank fired at +26s before the user could reproduce.
    let menu_id = unsafe { safe_read_u16(option_window + 0x180) }.unwrap_or(u16::MAX);
    let cs_menu_man = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        ))
    }
    .unwrap_or(0);
    let flag = if menu_id < 0x47 && cs_menu_man >= OPTIONSETTING_WINDOW_MIN_PTR {
        unsafe { safe_read_u8(cs_menu_man + 0x90 + menu_id as usize) }.unwrap_or(0)
    } else {
        0
    };
    let actively_shown = (flag & OPTIONSETTING_FLAG_ACTIVELY_SHOWN_BIT) != 0;
    if actively_shown && cur_is_display && cur_visible {
        OPTIONSETTING_CURRENT_PANE_EVER_VISIBLE.store(1, Ordering::SeqCst);
    }
    let ever_visible = OPTIONSETTING_CURRENT_PANE_EVER_VISIBLE.load(Ordering::SeqCst) != 0;

    // Which tab is the user on: SettingTabControl (window+0x1870) -> tab view (+0x10) -> index (+0xd4).
    let tab_view = unsafe {
        safe_read_usize(
            option_window + OPTIONSETTING_TAB_CONTROL_OFFSET + OPTIONSETTING_TAB_VIEW_OFFSET,
        )
    }
    .unwrap_or(0);
    let current_tab = if tab_view >= OPTIONSETTING_WINDOW_MIN_PTR {
        unsafe { safe_read_i32(tab_view + OPTIONSETTING_TAB_VIEW_SELECTED_INDEX_OFFSET) }
            .map(|v| v as usize)
            .unwrap_or(usize::MAX)
    } else {
        usize::MAX
    };
    OPTIONSETTING_CURRENT_TAB.store(current_tab, Ordering::SeqCst);
    if actively_shown {
        OPTIONSETTING_LAST_ACTIVE_TAB.store(current_tab, Ordering::SeqCst);
    } else {
        OPTIONSETTING_LAST_ACTIVE_TAB.store(usize::MAX, Ordering::SeqCst);
    }
    unsafe { sample_optionsetting_active_row_table(current_dialog, current_tab, actively_shown) };

    // Old (mislabeled) signature -- kept only as a secondary diagnostic; it is a constant, not the bug.
    let named_blank = wl.visible && visible_mask == 0;
    // Real blank: a healthy pane was seen earlier, and now the actively-shown current pane is hidden.
    let real_blank =
        ever_visible && actively_shown && current_dialog != 0 && cur_is_display && !cur_visible;

    // FIX: when the currently-selected tab's real pane is blank, run the native tab-select refresh for
    // that current tab, not just SetVisible. Manual SetVisible was disproven: it increments the fix
    // counter while DisplayInfo.Visible remains false and the stale Quit visual list can stay over Game
    // Options. Before calling native select, repair composite+0xb8 to current_dialog so its state-copy
    // step is self-copy instead of stale Quit->Game.
    if real_blank
        && current_tab < OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT
        && let Ok(select_addr) = game_rva(OPTIONSETTING_DIALOG_REFRESH_SELECTED_ROW_RVA)
    {
        unsafe {
            *((composite + OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET) as *mut usize) =
                current_dialog;
        }
        let select_tab: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(select_addr) };
        unsafe { select_tab(composite, current_tab as i32) };
        OPTIONSETTING_PANE_FIX_APPLIED.fetch_add(1, Ordering::SeqCst);
    }

    OPTIONSETTING_PANE_LAST_WINDOWLIST_RESOLVED.store(wl.resolved as usize, Ordering::SeqCst);
    OPTIONSETTING_PANE_LAST_WINDOWLIST_VISIBLE.store(wl.visible as usize, Ordering::SeqCst);
    OPTIONSETTING_PANE_LAST_DATATYPE.store(wl.datatype as u32 as usize, Ordering::SeqCst);
    OPTIONSETTING_PANE_LAST_RESOLVED_MASK.store(resolved_mask, Ordering::SeqCst);
    OPTIONSETTING_PANE_LAST_VISIBLE_MASK.store(visible_mask, Ordering::SeqCst);
    OPTIONSETTING_PANE_COMPOSITE_BOUND.store(composite_bound as usize, Ordering::SeqCst);
    OPTIONSETTING_CURRENT_DIALOG.store(current_dialog, Ordering::SeqCst);
    OPTIONSETTING_CURRENT_PANE_VISIBLE.store(cur_visible as usize, Ordering::SeqCst);
    OPTIONSETTING_CURRENT_PANE_DATATYPE.store(cur_dt as u32 as usize, Ordering::SeqCst);
    OPTIONSETTING_ACTIVELY_SHOWN.store(actively_shown as usize, Ordering::SeqCst);
    OPTIONSETTING_LAST_FLAG.store(flag as usize, Ordering::SeqCst);
    if named_blank {
        OPTIONSETTING_PANE_BLANK_DETECTED_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    if real_blank {
        OPTIONSETTING_REAL_BLANK_DETECTED_COUNT.fetch_add(1, Ordering::SeqCst);
        OPTIONSETTING_CURRENT_TAB_AT_BLANK.store(current_tab, Ordering::SeqCst);
    }
    let n = OPTIONSETTING_PANE_SAMPLE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    if n <= OPTIONSETTING_PANE_SAMPLE_LOG_CAP || real_blank {
        append_autoload_debug(format_args!(
            "optionsetting-pane: sample #{n} window=0x{option_window:x} tab={current_tab} flag=0x{flag:x} actively_shown={actively_shown} current_dialog=0x{current_dialog:x} current_pane(display={cur_is_display} visible={cur_visible} dt=0x{:x}) ever_visible={ever_visible} real_blank={real_blank} | named(wl_visible={} mask=0x{visible_mask:x} named_blank={named_blank}) guard_skips={}",
            cur_dt as u32,
            wl.visible,
            OPTIONSETTING_PANE_GUARD_SKIPS.load(Ordering::SeqCst)
        ));
    }
}

/// Post-original MenuWindowJob::Run work for System->Quit: System/ProfileSelect resource mapping + the
/// real-system-window hide, the in-world-load abort + return-title submit that actually complete a profile
/// switch, and save-picker pump maintenance. Extracted from the hook body so the winning MenuWindowJob::Run
/// detour can run it too: `title_custom_cover_menu_window_run_hook` and this hook both target the same RVA,
/// MinHook installs only one, so this hook's own install fails `MH_ERROR_ALREADY_CREATED` and none of this
/// would otherwise run (2026-07-15 root cause: dead hook -> profile load never completes + System menu never
/// hidden). `title_custom_cover_menu_window_run_hook` calls this after it runs the original.
pub(crate) unsafe fn system_quit_menu_window_run_post(job: usize, ret: usize) {
    let finalized_profile = system_windows::take_finalized_profile_select();
    if finalized_profile != 0
        && let Ok(base) = game_module_base()
    {
        // A pick owns its own close. Picking a save file closes this window on purpose and queues a
        // reopen as the slot view (`SAVE_PICKER_OPEN_SLOTS_PENDING`, resubmitted further down this
        // same function). Restoring the System windows here would be a second owner of the same
        // close, and it wins simply by running first: `system_quit_restore_real_system_windows`
        // resets the ProfileSelect state, which clears both the pending flag and the System dialog
        // the resubmit needs, so the resubmit below then finds nothing pending and never fires. That
        // is why picking an `.sl2` landed back on the Quit menu instead of the character list -- the
        // live log shows the finalizer restore at `+161525ms` and no resubmit line at all after it.
        //
        // So the restore runs only for a close nobody claimed: a real backout. Leaving the hide
        // state up is also what the reopen wants -- the window is coming straight back.
        if save_picker_resubmit_pending() {
            append_autoload_debug(format_args!(
                "system-quit-dup: skipped finalizer restore for window=0x{finalized_profile:x}; a picker resubmit owns this close"
            ));
        } else {
            unsafe {
                system_quit_restore_real_system_windows(
                    base,
                    "restore-real-profile-native-finalizer",
                )
            };
        }
    }
    let filename_ptr = unsafe { safe_read_usize(job + 0x60) }.unwrap_or(0);
    let filename = system_quit_read_wide_resource_name(filename_ptr);
    // Two fields, two resource names, two placements. The link field used to pass the path
    // editor's cache key, so both windows arrived here under one name and had to be told apart by
    // `build_url_keyboard_active()` -- with the Quit tab's field then getting no placement at all,
    // because the picker's helper positions against the ProfileSelect row layout. It now has its
    // own key, its own derived movie (chrome kept, box centred) and its own placement, and the
    // filename alone separates them. Routing stays split for the other reason too: the picker's
    // stale-window watchdog must never see the link field's window and conclude its own job went
    // quiet.
    if filename == save_picker_path_editor::TEXT_INPUT_RESOURCE_NAME {
        let owner =
            unsafe { safe_read_usize(job + MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET) }.unwrap_or(0);
        let state = if owner != 0 {
            unsafe { safe_read_i32(owner + MSGBOX_JOB_RESULT_STATE_1E8_OFFSET) }.unwrap_or_default()
        } else {
            0
        };
        // The PICKER's field only. The link field no longer reaches this branch at all: it carries
        // its own resource name since 2026-08-23, so the two windows are separated by the game's
        // own filename rather than by asking which editor claims the owner. That also keeps the
        // picker's stale-window watchdog from ever seeing the link field's window and concluding
        // its own job went quiet.
        if owner != 0
            && save_picker_note_path_editor_window_state(owner, state)
            && let Ok(base) = game_module_base()
        {
            unsafe { apply_path_editor_window_position(base, owner) };
        }
    }
    if filename == save_picker_path_editor::BUILD_URL_TEXT_INPUT_RESOURCE_NAME {
        let owner =
            unsafe { safe_read_usize(job + MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET) }.unwrap_or(0);
        if owner != 0 {
            let state = unsafe { safe_read_i32(owner + MSGBOX_JOB_RESULT_STATE_1E8_OFFSET) }
                .unwrap_or_default();
            if build_url_note_editor_window_state(owner, state)
                && let Ok(base) = game_module_base()
            {
                unsafe { apply_build_url_editor_window_position(base, owner) };
                // A window is only worth touching while it is still running; a terminal result
                // means its SceneObjProxy teardown has begun and a resolve would hand back
                // released objects. The picker's own state note already applies that rule, so the
                // link field applies the same one rather than inventing a second answer. This is
                // the per-frame work the field needs beyond placement -- the end-caret, and the
                // live clipboard mirror that lets a paste land in an already-open field.
                #[cfg(feature = "build-rows")]
                if text_input_02_990_window_is_live(state) {
                    unsafe { build_url_editor_window_run(base, owner) };
                }
            }
        }
    }
    if matches!(
        filename.as_str(),
        "02_000_IngameTop"
            | "02_040_OptionSetting"
            | "02_041_OptionSetting_Trial"
            | "05_010_ProfileSelect"
    ) {
        let owner = unsafe { safe_read_usize(job + 0x130) }.unwrap_or(0);
        let owner_vt = if owner != 0 {
            unsafe { safe_read_usize(owner) }.unwrap_or(0)
        } else {
            0
        };
        let owner_id = if owner != 0 {
            unsafe { safe_read_u16(owner + 0x180) }.unwrap_or(u16::MAX)
        } else {
            u16::MAX
        };
        let list = unsafe { safe_read_usize(job + 0x50) }.unwrap_or(0);
        let prev = match filename.as_str() {
            "02_000_IngameTop" => {
                // One tick per presented frame of the in-world PAUSE/SYSTEM menu. This branch is
                // the only place in the process that knows, by the game's own resource name, that
                // the menu Escape opens is up right now -- and it already runs here. The post-
                // release cover watch reads the resulting stamp to say how long after the user's
                // press a cover plate came back, instead of leaving that interval to be paired up
                // by hand from the log (2026-08-22 report).
                crate::telemetry::in_game_menu_note_run_tick(job, owner);
                SYSTEM_QUIT_INGAME_TOP_WINDOW.swap(owner, Ordering::SeqCst)
            }
            "02_040_OptionSetting" | "02_041_OptionSetting_Trial" => {
                SYSTEM_QUIT_OPTION_SETTING_WINDOW.swap(owner, Ordering::SeqCst)
            }
            "05_010_ProfileSelect" => {
                // One tick per rendered frame of our view. The live editor's safety gate reads this
                // to answer "is the ProfileSelect view on screen right now", which decides whether a
                // web-UI edit may be applied from the async FrameBegin path or has to wait for the
                // in-band row populate. Stamped here because this hook is the per-frame run of that
                // window's MenuWindowJob; nothing else in the process is that direct about it.
                er_telemetry_core::counters::PROFILE_SELECT_WINDOW_RUN_TICKS
                    .fetch_add(1, Ordering::SeqCst);
                SYSTEM_QUIT_PROFILE_SELECT_WINDOW.swap(owner, Ordering::SeqCst)
            }
            _ => 0,
        };
        let log_idx = SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_LOG_COUNT.fetch_add(1, Ordering::SeqCst);
        if log_idx < 64 || filename == "05_010_ProfileSelect" {
            append_autoload_debug(format_args!(
                "system-quit-dup: MenuWindowJob::Run resource='{filename}' job=0x{job:x} owner=0x{owner:x} owner_vt=0x{owner_vt:x} owner_id=0x{owner_id:x} prev=0x{prev:x} list_field=0x{list:x} ret=0x{ret:x}"
            ));
        }
        // Read-only oracle: on Game-Options (re-)entry, sample whether the option-row pane display
        // objects are actually visible (blank Game Options pane detector). Runs here because this hook
        // is the menu/game thread required for the GFx DisplayInfo vcalls. No game state is mutated.
        if matches!(
            filename.as_str(),
            "02_040_OptionSetting" | "02_041_OptionSetting_Trial"
        ) && owner != 0
            && let Ok(base) = game_module_base()
        {
            unsafe { sample_optionsetting_pane_visibility(base, owner) };
            // The one place in this process that holds a live `CS::OptionSettingTopDialog`: it is
            // read from `job+0x130` on the frame that job is running, so there is no stored pointer
            // to go stale. A build import applied from the Quit tab arms a portrait refresh and
            // this consumes it, then keeps sampling the verify window. It re-proves the window's
            // class before touching anything, so the other OptionSetting resource name and any
            // foreign owner cost a refusal rather than a corrupted object.
            unsafe { er_profile_summary_core::quit_panel_portrait_tick(owner) };
        }
        if filename == "05_010_ProfileSelect"
            && let Ok(base) = game_module_base()
        {
            if owner == 0 {
                // Picker navigation/pick closes the window with a queued resubmit; keep the
                // System UI hidden and let the resubmit block below reopen 05_010 instead of
                // restoring (a restore here would clobber the staged rows and flash the
                // System menu between pages).
                if !save_picker_resubmit_pending() {
                    unsafe {
                        system_quit_restore_real_system_windows(
                            base,
                            "restore-real-profile-owner-cleared",
                        )
                    };
                }
            } else {
                unsafe {
                    system_quit_hide_real_system_windows(base, "hide-real-after-profile-select-run")
                };
                // Menu-pump-owned cursor park. A foreign save's preview asked for the cursor to sit
                // on the lowest slot that save occupies; this is the first frame of the dialog that
                // shows it, so the rows exist and the game's own
                // `ProfileLoadDialog::SelectSaveSlot` can find one. Retried each frame until it
                // takes (an early frame can run before the row list is filled) and consumed on
                // success, so a user who then moves the cursor is never fought.
                unsafe { system_quit_park_profile_select_cursor(base, owner) };
            }
        }
    }
    // Abort the half-started in-world load transition. Pressing OK on ProfileSelect natively arms
    // GameMan.saveState/b80=2 (in-world load via deserialize 0x67b290) before any hook we control; our
    // load guard skips the deserialize so nothing loads, but the game still advances to saveState=3
    // ("loading") and sticks at a loading screen -- and that stuck load blocks the game/menu pump from
    // running the queued return-title chain (observed: functor_call_count=0, player still present).
    // While the first-world System-Quit transition is active and the old world is still up (local
    // player present), force saveState back to idle (0) so the load machine stops and the return-title
    // can run. Range-gated on [confirmed, AUTOLOAD_HANDOFF) -- Not `!= IDLE`: the clean-title reload runs
    // at AUTOLOAD_HANDOFF, and its own deserialize allocates a new PlayerIns so `local_player_mut()`
    // flips back to Ok (world_up=true). A `!= IDLE` gate would reopen here and zero the reload's own
    // saveState=2/3 mid-deserialize, yanking the load out from under a half-built FE/player -> the native
    // GFx text setter then dispatches the uninitialized object (the +39672ms garbage-vtable AV on the
    // 2nd in-process load). Excluding AUTOLOAD_HANDOFF leaves the reload's load untouched, exactly like a
    // boot autoload (phase idle, this branch never fires). Plain field write (not a menu/Scaleform call)
    // -> safe from the menu pump. See bd system-quit-load-profile-NOCRASH-milestone-2026-07-01.
    let sq_abort_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&sq_abort_phase)
        && unsafe { PlayerIns::local_player_mut() }.is_ok()
    {
        let gm = game_man_ptr_or_null();
        if gm != 0 && gm != TITLE_OWNER_SCAN_START_ADDRESS {
            let ss_ptr = (gm + GAME_MAN_SAVE_STATE_B80_OFFSET) as *mut i32;
            if let Some(ss) = unsafe { safe_read_i32(gm + GAME_MAN_SAVE_STATE_B80_OFFSET) }
                && (ss == GAME_MAN_SAVE_STATE_READING || ss == FULLREAD_B80_RESIDENT)
            {
                unsafe { *ss_ptr = GAME_MAN_SAVE_STATE_IDLE };
                let n = SYSTEM_QUIT_INWORLD_LOAD_ABORT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= 8 || n.is_multiple_of(120) {
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: aborted stuck in-world load transition #{n} saveState={ss}->0 (old world still up) so return-title chain can run"
                    ));
                }
            }
        }
    }
    // Menu-pump-owned save-flow confirm box (save-game-flow WP2): the game-task tick decides
    // which box comes next but must not build/submit a MenuJob, so it stages the box id here
    // and this -- the game's own menu pump executing a MenuWindowJob, the same context the
    // save-picker resubmit and the return-title chain use -- performs the submit. A failed
    // submit keeps the pending latch set so the next pump retries (the common cause is the
    // dialog's job queue still owning the previous box's job).
    let pending_box = SAVE_FLOW_SUBMIT_BOX_PENDING.load(Ordering::SeqCst);
    if pending_box != SAVE_FLOW_BOX_NONE && unsafe { save_flow_submit_box(pending_box) } {
        SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    }
    // Menu-pump-owned destination browser open (save-game-flow WP3): Box2 "No" means "save
    // somewhere else", which the tick stages here because opening the picker stages records and
    // submits a MenuJob -- menu-pump work, not game-task work.
    //
    // What clears the latch is "A PICKER RAN", not "A PICKER IS UP". Retrying only makes sense for
    // an open that never happened -- a MenuJob the dialog's queue deferred. A picker that ran and
    // came back with no destination has answered this request, and re-arming it re-asks a question
    // the user just declined: with the OS surface that reopened comdlg32 ~57 ms after every Cancel,
    // forever, with no way out of the flow (bd `er-effects-rs-rsxi`). The tick's OpenTimeout could
    // not save it either, because each reopen blocks the whole frame, so the budget never accrued.
    if SAVE_DEST_OPEN_PICKER_PENDING.load(Ordering::SeqCst) != 0 {
        let system_dialog = SAVE_FLOW_DIALOG.load(Ordering::SeqCst);
        if unsafe { system_quit_open_save_dest_picker(system_dialog) }.request_discharged() {
            SAVE_DEST_OPEN_PICKER_PENDING.store(0, Ordering::SeqCst);
        } else {
            // The one path that legitimately re-arms. Counted so a run can prove which one it took:
            // in OS mode this must stay 0, and any positive value is the reopen loop returning.
            SAVE_DEST_PICKER_OPEN_RETRY_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }
    // Menu-pump-owned save-picker maintenance: drive-cell input, native ScrollBarV sync,
    // edge-scroll restaging, in-place row rebuild after navigation, and window resubmit after a
    // navigation/pick close (same submit-context rule as the return-title chain below).
    unsafe { save_picker_menu_pump_path_editor() };
    // Menu-pump-owned build-url link field. Same context and same reason as the path editor above:
    // it builds and submits a native SoftwareKeyboardJob, which must not happen on the game task.
    #[cfg(feature = "build-rows")]
    unsafe {
        build_url_editor_menu_pump()
    };
    unsafe { save_picker_menu_pump_drive_strip_mouse() };
    unsafe { save_picker_menu_pump_native_scrollbar() };
    unsafe { save_picker_menu_pump_edge_scroll() };
    unsafe { save_picker_menu_pump_rebuild() };
    if save_picker_resubmit_pending() {
        let _ = unsafe { save_picker_menu_pump_resubmit() };
    }
    // Menu-pump-owned return-title submit. This hook is the game's menu pump executing a
    // MenuWindowJob, so submitting the return-title chain from here (rather than from the concurrent
    // game-task tick) runs it in the menu pump's own frame and eliminates the Scaleform race that
    // produced the non-deterministic execute-fault crashes. Fire once ProfileSelect has closed (its
    // window cleared) during the return-title teardown window only; after AUTOLOAD_HANDOFF the picked
    // slot's SetState5/MoveMap stream owns the session and a second return-title request would leave
    // GameMan+0xbc4=3 stale, blocking the incoming world's MoveMap(18) finalize.
    // See bd system-quit-return-title-scaleform-race-2026-07-01 and er-effects-rs-um9g.
    let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&quickload_phase)
        && SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) == 0
        && SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) == 0
        && let Ok(base) = game_module_base()
    {
        let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
        if system_dialog != 0 && system_dialog != TITLE_OWNER_SCAN_START_ADDRESS {
            let _ = unsafe {
                system_quit_submit_direct_return_title_chain(
                    base,
                    system_dialog,
                    "menu-pump-run-hook",
                )
            };
        }
    }
}
