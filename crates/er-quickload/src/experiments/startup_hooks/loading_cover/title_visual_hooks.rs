//! The title's own visuals: what the loading cover hides, and the character stats it draws.
//!
//! These seven hooks stood in `quit_menu/profile_rows_system_quit_menu.rs` because that file was
//! where the System>Quit work happened to be written, not because the rows own them. None of them
//! reads a cloned row or a save container: they hide the logo, the `PRESS ANY BUTTON` line and the
//! native title menu so the cover can be drawn over a clean screen, and they push `ErCharStats`
//! into the title's row-populate so the stats panel has text. Their own log prefixes say so --
//! `title-cover-part-a:` and `stats-text:` -- and every call site is
//! `lifecycle/title_visual_startup.rs`.
//!
//! Pure move, no behaviour change. The module tree is what a `#[cfg(feature = ...)]` can be put
//! on, and it could not go on a directory holding five features' code.

use super::*;

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
