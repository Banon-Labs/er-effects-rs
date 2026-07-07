/// Install the row-populate hook (`FUN_1408758d0`). Idempotent; mirrors the named-child binder install.
pub(crate) fn install_profile_row_populate_hook() {
    if PROFILE_ROW_POPULATE_INSTALLED.load(Ordering::SeqCst) != 0 {
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
    let Ok(addr) = game_rva(PROFILE_ROW_POPULATE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "stats-text: failed to resolve row-populate rva 0x{PROFILE_ROW_POPULATE_RVA:x}"
        ));
        return;
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
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
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
    let forced_visible = if (single_target != null && single_target != 0 && value == single_target)
        || in_text_hide_set
    {
        TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE.store(value, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED.store(visible as usize, Ordering::SeqCst);
        0
    } else {
        visible
    };
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
    let Ok(addr) = game_rva(TITLE_GFX_VALUE_SET_VISIBLE_RVA as u32) else {
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
                    std::mem::forget(hook);
                    TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked GFx visibility setter 0x{addr:x}; only PressStart value will be forced hidden"
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

pub(crate) fn install_title_custom_cover_run_hook() {
    if TITLE_CUSTOM_COVER_RUN_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: MenuWindowJob::Run MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(run_addr) = game_rva(MENU_WINDOW_JOB_RUN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-b: failed to resolve MenuWindowJob::Run rva 0x{MENU_WINDOW_JOB_RUN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            run_addr as *mut c_void,
            title_custom_cover_menu_window_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_CUSTOM_COVER_RUN_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-b: queue_enable MenuWindowJob::Run failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_CUSTOM_COVER_RUN_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-b: hooked MenuWindowJob::Run 0x{run_addr:x}; ProfileSelect cover will run alongside preserved native title job"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-b: MenuWindowJob::Run MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-b: MhHook::new MenuWindowJob::Run failed: {status:?}"
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
                        std::mem::forget(hook);
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
                        std::mem::forget(hook);
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
    let Ok(start_login_addr) = game_rva(TITLE_TOP_START_LOGIN_RVA as u32) else {
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
                    std::mem::forget(hook);
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
    let Ok(addr) = game_rva(TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA as u32) else {
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
                    std::mem::forget(hook);
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
    let Ok(begin_title_addr) = game_rva(TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA as u32) else {
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
                    std::mem::forget(hook);
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
    let Ok(fadein_addr) = game_rva(TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RVA as u32) else {
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
                    std::mem::forget(hook);
                    TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED.store(
                        TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked MenuWindowJob FadeIn helper 0x{fadein_addr:x}; preserved native {TITLE_NATIVE_MENU_VISUAL_NAME} will clear visible flags mask 0x{TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK:x} from CSMenuMan+0x90 when Run returns at rva 0x{TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RUN_CALLER_RVA:x}"
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

#[repr(C, align(8))]
struct SystemQuitMenuHelpLabelScratch {
    bytes: [u8; MENU_HELP_LABEL_SIZE],
}

#[repr(C, align(8))]
struct SystemQuitRootProxyScratch {
    bytes: [u8; MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE],
}

fn system_quit_list_slot_addr(list: usize, slot: usize) -> usize {
    list.wrapping_add((0usize.wrapping_sub(list)) & 7)
        .wrapping_add(slot * std::mem::size_of::<usize>())
}

unsafe fn system_quit_menu_window_set_visible_and_flags(
    base: usize,
    window: usize,
    visible: bool,
    source: &str,
) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if window < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- window=0x{window:x} not heap-like"
        ));
        return false;
    }
    let window_vt = unsafe { safe_read_usize(window) }.unwrap_or(NULL);
    if window_vt < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- window=0x{window:x} vt=0x{window_vt:x} invalid"
        ));
        return false;
    }
    let mut scratch = SystemQuitRootProxyScratch {
        bytes: [0; MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE],
    };
    let Ok(root_proxy_ctor_addr) = game_rva(MENU_WINDOW_ROOT_PROXY_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve root proxy ctor rva 0x{MENU_WINDOW_ROOT_PROXY_CTOR_RVA:x}"
        ));
        return false;
    };
    let Ok(set_visible_addr) = game_rva(TITLE_PRESS_START_SET_VISIBLE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve SetVisible rva 0x{TITLE_PRESS_START_SET_VISIBLE_RVA:x}"
        ));
        return false;
    };
    let Ok(dtor_addr) = game_rva(MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve root proxy scratch dtor rva 0x{MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA:x}"
        ));
        return false;
    };
    let root_proxy_ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(root_proxy_ctor_addr) };
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(set_visible_addr) };
    let dtor: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(dtor_addr) };
    let scratch_ptr = scratch.bytes.as_mut_ptr() as usize;
    let root_proxy = unsafe { root_proxy_ctor(window, scratch_ptr) };
    if root_proxy != scratch_ptr {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window root-proxy ctor returned unexpected 0x{root_proxy:x} scratch=0x{scratch_ptr:x}; still using returned proxy"
        ));
    }
    unsafe { set_visible(root_proxy, u8::from(visible)) };
    unsafe { dtor(scratch_ptr + 0x28) };

    let menu_id = unsafe { safe_read_u16(window + 0x180) }.unwrap_or(u16::MAX);
    let cs_menu_man = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }.unwrap_or(NULL);
    let mut flags_before = NULL;
    let mut flags_after = NULL;
    if menu_id < 0x47 && cs_menu_man >= HEAP_LO {
        let flags_addr = cs_menu_man + 0x90 + menu_id as usize;
        if let Some(flags) = unsafe { safe_read_u8(flags_addr) } {
            flags_before = flags as usize;
            let new_flags = if visible {
                flags | TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK
            } else {
                flags & 1
            };
            unsafe { (flags_addr as *mut u8).write_volatile(new_flags) };
            flags_after = new_flags as usize;
        }
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: {source} top-window visibility window=0x{window:x} vt=0x{window_vt:x} visible={visible} root_proxy=0x{root_proxy:x} menu_id=0x{menu_id:x} flags=0x{flags_before:x}->0x{flags_after:x}"
    ));
    true
}

fn system_quit_read_wide_resource_name(ptr: usize) -> String {
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

unsafe fn system_quit_hide_real_system_windows(base: usize, source: &str) {
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if profile == 0 || SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0 {
        return;
    }
    let hid_top = if top != 0 && top != profile {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, top, false, source) }
    } else {
        false
    };
    let hid_option = if option != 0 && option != profile && option != top {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, option, false, source) }
    } else {
        false
    };
    if hid_top || hid_option {
        SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.store(1, Ordering::SeqCst);
        SYSTEM_QUIT_HIDE_REAL_WINDOWS_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: real-system-window hide source={source} top=0x{top:x} option=0x{option:x} profile=0x{profile:x} hid_top={hid_top} hid_option={hid_option}"
    ));
}

/// Re-apply the OptionSetting active-pane visibility via the game's OWN tab-select visibility pass
/// (`FUN_14093b850`, deobf 0x93b760). Our hide/restore of the OptionSetting window leaves every option
/// pane with `DisplayInfo.Visible=0` (proven by oracle_optionsetting_pane_visible: WindowList visible,
/// visible_mask=0 -> the blank Game Options pane). Driving the window's show state does NOT re-show the
/// panes -- pane visibility is applied only by this per-tab pass, which our restore never re-triggers.
/// We derive the current tab index by matching the composite's current pane dialog (`composite+0xb8`)
/// against the 10-entry cache at `composite+0x68`, then call the pass so it re-runs
/// `SetVisible(paneDialog+0x1200, current==dialog)` for every cached pane -- re-showing exactly the
/// active pane. Runs on the menu thread (the restore path is menu-pump owned). Read-guarded; no-ops if
/// the composite / current pane / tab index can't be resolved.
unsafe fn system_quit_reapply_optionsetting_pane_visibility(base: usize, option_window: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if option_window < HEAP_LO {
        return;
    }
    let menu_id = unsafe { safe_read_u16(option_window + 0x180) }.unwrap_or(u16::MAX);
    if menu_id != OPTIONSETTING_MENU_ID {
        // Not the OptionSetting window (e.g. the IngameTop top-menu, menu_id 0xffff) -- this composite
        // layout is OptionSetting-specific; skip.
        return;
    }
    let composite = option_window + OPTIONSETTING_COMPOSITE_OFFSET;
    let current = unsafe {
        safe_read_usize(composite + OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET)
    }
    .unwrap_or(0);
    if current < HEAP_LO {
        return;
    }
    // The REAL selected tab the user is viewing: SettingTabControl at window+0x1870, its tab view at
    // +0x10, selected index at view+0xd4 (`FUN_140739f20` = `*(view+0xd4)`). Use THIS, not the composite's
    // `current` pane pointer -- after our detour `current` (composite+0xb8) is stale (observed: it matched
    // cache slot 9 while the user was on the Game tab), so re-applying its index re-shows the wrong pane.
    const TAB_CONTROL_OFFSET: usize = 0x1870;
    const TAB_VIEW_OFFSET: usize = 0x10;
    const TAB_VIEW_SELECTED_INDEX_OFFSET: usize = 0xd4;
    let tab_view =
        unsafe { safe_read_usize(option_window + TAB_CONTROL_OFFSET + TAB_VIEW_OFFSET) }.unwrap_or(0);
    let real_tab = if tab_view >= HEAP_LO {
        unsafe { safe_read_i32(tab_view + TAB_VIEW_SELECTED_INDEX_OFFSET) }
            .map(|v| v as usize)
            .filter(|&t| t < OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT)
    } else {
        None
    };
    // Diagnostic: which cache slot the (possibly stale) current pane pointer matches.
    let mut cache_tab: Option<usize> = None;
    for i in 0..OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT {
        let cached = unsafe {
            safe_read_usize(composite + OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET + i * 8)
        }
        .unwrap_or(0);
        if cached == current {
            cache_tab = Some(i);
            break;
        }
    }
    let Some(tab_index) = real_tab.or(cache_tab) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: optionsetting pane-reapply skipped -- no tab index (tab_view=0x{tab_view:x} current=0x{current:x} composite=0x{composite:x})"
        ));
        return;
    };
    let Ok(select_addr) = game_rva(OPTIONSETTING_TAB_SELECT_VISIBILITY_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: optionsetting pane-reapply skipped -- select rva 0x{OPTIONSETTING_TAB_SELECT_VISIBILITY_RVA:x} unresolved"
        ));
        return;
    };
    let select: unsafe extern "system" fn(usize, i32, usize, usize) =
        unsafe { std::mem::transmute(select_addr) };
    unsafe { select(composite, tab_index as i32, NULL, NULL) };
    append_autoload_debug(format_args!(
        "system-quit-dup: optionsetting pane-reapply composite=0x{composite:x} current=0x{current:x} tab_index={tab_index} real_tab={real_tab:?} cache_tab={cache_tab:?} via 0x{select_addr:x}"
    ));
}

unsafe fn system_quit_reset_profile_select_state(source: &str) {
    SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_LIST.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID.store(usize::MAX, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: reset ProfileSelect hide state source={source}"
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
        if waits <= 3 || waits % 60 == 0 {
            let head = unsafe { safe_read_usize(queue) }.unwrap_or(NULL);
            let pending6 = unsafe { safe_read_usize(queue + 0x30) }.unwrap_or(NULL);
            append_autoload_debug(format_args!(
                "system-quit-quickload: direct return-title chain WAIT source={source} waits={waits} queue not ready dialog=0x{system_dialog:x} queue=0x{queue:x} head=0x{head:x} field6=0x{pending6:x}"
            ));
        }
        return false;
    }
    // Fire the NATIVE return-title REQUEST (FUN_14067a490, live 0x67a3a0) -- the missing piece. It sets
    // GameMan.saveRequested = true and GameMan+0xbc4 = 1 (== GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY).
    // WITHOUT it, bc4 stays 0, so (a) the game never recognizes a return-to-title is pending and never
    // saves+tears down the world, and (b) our final functor (title.rs, gated on bc4==READY) never fires,
    // leaving the submitted chain job orphaned in a queue that stops being pumped once the menus close.
    // Observed 2026-07-01: OK -> menus closed but still in-world, same char, functor_call_count=0,
    // bc4=0, native_quit_action_count=0. The native Quit-Game does this request AND the build+submit
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
                append_autoload_debug(format_args!(
                    "system-quit-quickload: native return-title REQUEST fired 0x{req_addr:x} source={source} -- set saveRequested + bc4=1 so the world saves+tears down and the final functor can fire"
                ));
            }
            Err(_) => append_autoload_debug(format_args!(
                "system-quit-quickload: return-title request rva 0x{SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA:x} unresolved source={source}"
            )),
        }
    }
    let Ok(builder_addr) = game_rva(SYSTEM_QUIT_RETURN_TITLE_CHAIN_BUILDER_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- builder rva 0x{SYSTEM_QUIT_RETURN_TITLE_CHAIN_BUILDER_RVA:x} unresolved"
        ));
        return false;
    };
    let Ok(submit_addr) = game_rva(MENU_JOB_SUBMIT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- submit rva 0x{MENU_JOB_SUBMIT_RVA:x} unresolved"
        ));
        return false;
    };
    let builder: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(builder_addr) };
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(submit_addr) };
    let mut job_slot: usize = 0;
    let job_slot_ptr = (&raw mut job_slot) as usize;
    unsafe { builder(job_slot_ptr, list) };
    let job = job_slot;
    if job < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain builder produced no plausible job source={source} dialog=0x{system_dialog:x} list=0x{list:x} job=0x{job:x}"
        ));
        return false;
    }
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-quickload: direct return-title chain SUBMIT source={source} builder=0x{builder_addr:x} submit=0x{submit_addr:x} dialog=0x{system_dialog:x} queue=0x{queue:x} list=0x{list:x} job=0x{job:x}; waiting for real title menu rebuild before Continue fallback"
    ));
    unsafe { submit(queue, job_slot_ptr) };
    true
}

unsafe fn system_quit_restore_real_system_windows(base: usize, source: &str) {
    if SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) == 0 {
        unsafe { system_quit_reset_profile_select_state(source) };
        return;
    }
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
        let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
        let submitted =
            unsafe { system_quit_submit_direct_return_title_chain(base, system_dialog, source) };
        SYSTEM_QUIT_SKIP_RESTORE_AFTER_QUICKLOAD_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-dup: skip restore real windows after quickload handoff source={source} phase={phase} profile=0x{profile:x} top=0x{top:x} option=0x{option:x} direct_chain_submitted={submitted}; leaving old System UI hidden during native transition"
        ));
        if submitted {
            SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
            unsafe { system_quit_reset_profile_select_state(source) };
        }
        return;
    }
    let restored_top = if top != 0 {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, top, true, source) }
    } else {
        false
    };
    let restored_option = if option != 0 && option != top {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, option, true, source) }
    } else {
        false
    };
    if restored_option {
        // Showing the OptionSetting window root does NOT re-show its option panes -- pane visibility is
        // applied only by the game's per-tab pass, which our hide/restore never re-triggers, leaving the
        // Game Options tab blank. Re-run that pass for the active tab so the current pane re-shows.
        unsafe { system_quit_reapply_optionsetting_pane_visibility(base, option) };
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: restore real windows source={source} profile=0x{profile:x} top=0x{top:x} option=0x{option:x} restored_top={restored_top} restored_option={restored_option}"
    ));
    unsafe { system_quit_save_swap_restore_profile_summary(source) };
    unsafe { system_quit_reset_profile_select_state(source) };
    if restored_top || restored_option {
        SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) unsafe fn system_quit_profile_select_top_menu_tick() {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let hidden = SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if !hidden {
        return;
    }
    if profile == 0 {
        // ProfileSelect has closed. Do NOT submit the return-title chain from this game-task tick:
        // that runs concurrently with the game's own menu/Scaleform pump and corrupts it (observed:
        // non-deterministic execute-fault jumping into Scaleform string data). The close is done in
        // menu-pump ownership by the native confirm transition (dialog+0x1e8=Success pops the
        // ProfileSelect window job) and the return-title submit is done in menu-pump ownership from
        // the MenuWindowJob::Run hook. See bd system-quit-return-title-scaleform-race-2026-07-01.
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) == SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
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
                unsafe { system_quit_reset_profile_select_state("profile-select-closed-without-load-no-base") };
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
    if let Ok(base) = game_module_base() {
        unsafe { system_quit_restore_real_system_windows(base, "restore-real-profile-left-list") };
    } else {
        unsafe { system_quit_reset_profile_select_state("restore-real-profile-left-list-no-base") };
    }
}

/// Result of resolving one named OptionSetting child and reading its DisplayInfo.Visible.
struct OptionSettingPaneSample {
    /// `assignComponentWithName` returned a live out proxy (not 0 / not the null sentinel).
    resolved: bool,
    /// The resolved child's CSScaleformValue is a live DisplayObject (`(dataType & MASK) == VALUE`).
    is_display: bool,
    /// DisplayInfo.Visible byte was nonzero after the `GetDisplayInfo` vcall.
    visible: bool,
    /// Raw dataType (for gate diagnosis when `is_display` is false).
    datatype: i32,
}

/// READ-ONLY: resolve one named child of the OptionSetting root proxy and read its
/// DisplayInfo.Visible. Mirrors `push_stats_text_on_row`'s resolve/guard/release exactly -- native
/// `assignComponentWithName` into a zeroed out proxy, the 7e7 game-image guard on the vptr chain
/// before any virtual dispatch, and `~CSScaleformValue` on the out proxy's EMBEDDED value (+0x28).
/// Nothing is mutated; the `GetDisplayInfo` vcall only fills the caller's stack buffer. dtor is run
/// exactly once for every resolved out proxy (never for an unresolved name).
unsafe fn resolve_optionsetting_pane(
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
    let out = unsafe { assign(root_proxy, out_buf.as_mut_ptr() as usize, name.as_ptr() as usize) };
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
/// `(is_display, visible, datatype)`. READ-ONLY: the `GetDisplayInfo` vcall only fills a local buffer;
/// this does NOT release the value (the caller owns lifetime -- an assign'd out proxy is dtor'd by the
/// caller; an embedded proxy has nothing to release). 7e7 guard on the vptr chain before any dispatch:
/// validate the vtable (`*objectInterface`) and the resolved fn are game-image-live (NOT the heap
/// objectInterface instance itself). `safe_read` of `*objectInterface` fails closed if unmapped.
unsafe fn read_scaleform_pane_visible(base: usize, cs_value: usize) -> (bool, bool, i32) {
    let object_interface =
        unsafe { safe_read_usize(cs_value + CSSCALEFORMVALUE_OBJECT_INTERFACE_OFFSET) }.unwrap_or(0);
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
        unsafe { safe_read_usize(vfptr + CSSCALEFORMVALUE_GET_DISPLAY_INFO_VTABLE_SLOT) }.unwrap_or(0)
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

/// READ-ONLY oracle: on OptionSetting menu re-entry, read whether the option-row pane display
/// objects are actually VISIBLE. Detects the "blank Game Options pane" bug (tab strip + footer
/// render, row list is black) with no screenshot -- every access is a read; no game state changes.
/// Runs on the menu/game thread (the `MenuWindowJob::Run` hook) as required for GFx vcalls.
unsafe fn sample_optionsetting_pane_visibility(base: usize, option_window: usize) {
    if option_window == 0 || option_window < OPTIONSETTING_WINDOW_MIN_PTR {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Prefer the hooked ORIG trampoline so the resolve is not double-instrumented (as in
    // push_stats_text_on_row); else the raw game RVA.
    let assign_addr = match TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG.load(Ordering::SeqCst) {
        orig if orig != null && orig != HOOK_ORIGINAL_UNSET => orig,
        _ => base + TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA,
    };
    let assign: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(assign_addr) };
    let dtor: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + CSSCALEFORMVALUE_DTOR_RVA) };
    let root_proxy = option_window + OPTION_SETTING_ROOT_PROXY_OFFSET;

    // The pane CONTAINER: its resolved-but-not-visible state IS the direct blank-pane signature.
    let wl = unsafe { resolve_optionsetting_pane(base, assign, dtor, root_proxy, OPTIONSETTING_WINDOWLIST_NAME) };

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

    // THE REAL SIGNAL: the game's tab-select (FUN_14093b850) toggles SetVisible on the CURRENT tab
    // dialog's embedded proxy at dialog+0x1200 -- NOT the named WindowList children (which stay
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
    let cs_menu_man = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }.unwrap_or(0);
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
        safe_read_usize(option_window + OPTIONSETTING_TAB_CONTROL_OFFSET + OPTIONSETTING_TAB_VIEW_OFFSET)
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

    // OLD (mislabeled) signature -- kept only as a secondary diagnostic; it is a constant, not the bug.
    let named_blank = wl.visible && visible_mask == 0;
    // REAL blank: a healthy pane was seen earlier, and now the actively-shown current pane is hidden.
    let real_blank = ever_visible && actively_shown && current_dialog != 0 && cur_is_display && !cur_visible;

    // FIX: the active tab's pane MUST be visible when the OptionSetting is shown. On return to a tab the
    // game's tab-select sometimes fails to re-show it (the blank Game Options pane -- proven: same dialog
    // 0x..564080 goes Visible=1 -> 0 and never comes back). Re-assert the invariant with the game's OWN
    // SetVisible on the current tab dialog's proxy at dialog+0x1200 -- the exact proxy/call FUN_14093b850
    // uses. Only when actively shown + the pane is a real DisplayObject + currently hidden, so it never
    // forces a pane that should legitimately be hidden (ProfileSelect clears flag 0x4 -> actively_shown=false).
    if actively_shown && cur_is_display && !cur_visible && current_dialog >= OPTIONSETTING_WINDOW_MIN_PTR
    {
        if let Ok(sv_addr) = game_rva(TITLE_PRESS_START_SET_VISIBLE_RVA as u32) {
            let set_visible: unsafe extern "system" fn(usize, u8) =
                unsafe { std::mem::transmute(sv_addr) };
            unsafe {
                set_visible(current_dialog + OPTIONSETTING_DIALOG_PANE_PROXY_OFFSET, 1);
            }
            OPTIONSETTING_PANE_FIX_APPLIED.fetch_add(1, Ordering::SeqCst);
        }
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

pub(crate) unsafe extern "system" fn system_quit_menu_window_job_run_hook(
    job: usize,
    load_params: usize,
    fd4_time: usize,
    menu_man: usize,
) -> usize {
    let orig = SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return load_params;
    }
    let filename_ptr = unsafe { safe_read_usize(job + 0x60) }.unwrap_or(0);
    let filename = system_quit_read_wide_resource_name(filename_ptr);

    // SEAMLESS ToS SKIP (2026-07-06): the online-service ToS (`06_000_TermOfService_BNE`) is a job in
    // the title `CS::FixOrderJobSequence`, which steps to the next job ONLY when a job's Run returns a
    // Success result. While the ToS window is up this job returns Continue (and the ctor-null makes it
    // Failed), so the zero-input autoload stalls on it forever (observed +17s..+108s idle). The
    // ToS/Privacy policy is an OFFICIAL-servers-only gate the DLL never needs -- ERSC uses its own
    // private relay and does not respect the official policy, so skipping it is safe for co-op. Force
    // this one job's MenuJobResult to Success BEFORE running the original (so the ToS never builds --
    // no window, no MessageBox, zero input) and return; `FixOrderJobSequence::Run` then advances past
    // it. Same proven pattern as `show_progress_job_run_hook` advancing the network/login jobs. The
    // MenuJobResult is at `load_params+0` (Run returns `load_params`, read as `MenuJobResult*` by the
    // sequence). Gated by `policy_tos_suppress_enabled()` (product autoload + Seamless, or the diag
    // override), so vanilla-offline is untouched (the ToS never fires there anyway).
    // Record which job's Run is executing so the nested MessageBox builder hook can attribute a
    // (suppressed) ERSC popup to this job and latch it into MSGBOX_STALL_JOB for next-frame advance.
    CURRENT_MENU_WINDOW_JOB_RUN_JOB.store(job, Ordering::SeqCst);

    // Advance-skip a title job to Success so `FixOrderJobSequence::Run` steps past it (never showing
    // its modal). Two Seamless cases: (1) the official-servers ToS job (`06_000_TermOfService_BNE`);
    // (2) the ERSC post-PAB MessageBox job -- its dialog build was already nulled by
    // `msgbox_builder_hook`, which latched THIS job into `MSGBOX_STALL_JOB`, so its next Run advances.
    static SEAMLESS_TOS_SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
    let is_tos = filename.contains("TermOfService");
    let is_stalled_msgbox = job != 0 && MSGBOX_STALL_JOB.load(Ordering::SeqCst) == job;
    if policy_tos_suppress_enabled() && (is_tos || is_stalled_msgbox) {
        if is_stalled_msgbox {
            MSGBOX_STALL_JOB.store(0, Ordering::SeqCst);
        }
        const MENU_JOB_STATE_SUCCESS: i32 = 2;
        const FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA: usize = 0x29c8e48;
        if load_params != 0 {
            unsafe {
                *(load_params as *mut i32) = MENU_JOB_STATE_SUCCESS;
                *((load_params + 4) as *mut i32) = 0;
            }
        }
        if let Ok(base) = game_module_base() {
            if fd4_time != 0 {
                unsafe { *(fd4_time as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
            }
        }
        let skip_n = SEAMLESS_TOS_SKIP_COUNT.fetch_add(1, Ordering::SeqCst);
        if skip_n < 12 {
            let kind = if is_tos {
                "official-ToS"
            } else {
                "ersc-post-pab-msgbox"
            };
            append_autoload_debug(format_args!(
                "seamless-tos-skip #{skip_n} ({kind}): forced MenuWindowJob::Run('{filename}') -> MenuJobResult(Success) job=0x{job:x} -- FixOrderJobSequence advances past the never-shown modal"
            ));
        }
        return load_params;
    }

    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { original(job, load_params, fd4_time, menu_man) };
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
            "02_000_IngameTop" => SYSTEM_QUIT_INGAME_TOP_WINDOW.swap(owner, Ordering::SeqCst),
            "02_040_OptionSetting" | "02_041_OptionSetting_Trial" => {
                SYSTEM_QUIT_OPTION_SETTING_WINDOW.swap(owner, Ordering::SeqCst)
            }
            "05_010_ProfileSelect" => {
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
        // READ-ONLY oracle: on Game-Options (re-)entry, sample whether the option-row pane display
        // objects are actually VISIBLE (blank Game Options pane detector). Runs here because this hook
        // IS the menu/game thread required for the GFx DisplayInfo vcalls. No game state is mutated.
        if matches!(
            filename.as_str(),
            "02_040_OptionSetting" | "02_041_OptionSetting_Trial"
        ) && owner != 0
        {
            if let Ok(base) = game_module_base() {
                unsafe { sample_optionsetting_pane_visibility(base, owner) };
            }
        }
        if filename == "05_010_ProfileSelect" {
            if let Ok(base) = game_module_base() {
                if owner == 0 {
                    unsafe {
                        system_quit_restore_real_system_windows(
                            base,
                            "restore-real-profile-owner-cleared",
                        )
                    };
                } else {
                    unsafe {
                        system_quit_hide_real_system_windows(
                            base,
                            "hide-real-after-profile-select-run",
                        )
                    };
                }
            }
        }
    }
    // ABORT the half-started in-world load transition. Pressing OK on ProfileSelect natively arms
    // GameMan.saveState/b80=2 (in-world load via deserialize 0x67b290) BEFORE any hook we control; our
    // load guard skips the deserialize so nothing loads, but the game still advances to saveState=3
    // ("loading") and STICKS at a loading screen -- and that stuck load blocks the game/menu pump from
    // running the queued return-title chain (observed: functor_call_count=0, player still present).
    // While the FIRST-world System-Quit transition is active AND the old world is still up (local
    // player present), force saveState back to idle (0) so the load machine stops and the return-title
    // can run. RANGE-gated on [CONFIRMED, AUTOLOAD_HANDOFF) -- NOT `!= IDLE`: the clean-title reload runs
    // at AUTOLOAD_HANDOFF, and its OWN deserialize allocates a NEW PlayerIns so `local_player_mut()`
    // flips back to Ok (world_up=true). A `!= IDLE` gate would REOPEN here and zero the RELOAD's own
    // saveState=2/3 mid-deserialize, yanking the load out from under a half-built FE/player -> the native
    // GFx text setter then dispatches the uninitialized object (the +39672ms garbage-vtable AV on the
    // 2nd in-process load). Excluding AUTOLOAD_HANDOFF leaves the reload's load untouched, exactly like a
    // boot autoload (phase IDLE, this branch never fires). Plain field write (not a menu/Scaleform call)
    // -> safe from the menu pump. See bd system-quit-load-profile-NOCRASH-milestone-2026-07-01.
    let sq_abort_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&sq_abort_phase)
        && unsafe { PlayerIns::local_player_mut() }.is_ok()
    {
        let gm = game_man_ptr_or_null();
        if gm != 0 && gm != TITLE_OWNER_SCAN_START_ADDRESS {
            let ss_ptr = (gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *mut i32;
            if let Some(ss) = unsafe { safe_read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) } {
                if ss == 2 || ss == 3 {
                    unsafe { *ss_ptr = 0 };
                    let n = SYSTEM_QUIT_INWORLD_LOAD_ABORT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    if n <= 8 || n % 120 == 0 {
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: aborted stuck in-world load transition #{n} saveState={ss}->0 (old world still up) so return-title chain can run"
                        ));
                    }
                }
            }
        }
    }
    // MENU-PUMP-OWNED return-title submit. This hook IS the game's menu pump executing a
    // MenuWindowJob, so submitting the return-title chain from here (rather than from the concurrent
    // game-task tick) runs it in the menu pump's own frame and eliminates the Scaleform race that
    // produced the non-deterministic execute-fault crashes. Fire once ProfileSelect has closed (its
    // window cleared) during a return-title transition; the submit self-gates on queue-ready and
    // one-shots via the submit count. See bd system-quit-return-title-scaleform-race-2026-07-01.
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
        && SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) == 0
        && SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) == 0
    {
        if let Ok(base) = game_module_base() {
            let system_dialog =
                SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
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
    ret
}
