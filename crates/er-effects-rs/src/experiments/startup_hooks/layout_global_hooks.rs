
fn apply_system_quit_multislot_layout_patch() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch skipped -- module base unavailable"
        ));
        return;
    };
    let target = (base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing == SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch already applied at 0x{:x} value=0x{existing:x}",
            base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA
        ));
        return;
    }
    if existing != SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END:x}",
            base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            SYSTEM_QUIT_COMPONENT_INDEX_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch VirtualProtect failed"
        ));
        return;
    }
    unsafe {
        *target = SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            SYSTEM_QUIT_COMPONENT_INDEX_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: patched Quit Game component index 0x{:x} 0x{SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END:x}->0x{SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT:x} (multi-slot layout proof)",
        base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA
    ));
}

/// Install the System -> Quit Game duplicate-button proof hook once. The detour is a pass-through
/// for every `AddCancelButton` call except the first Quit Game tab row, where it invokes the
/// original trampoline again with native args for Load Profile and Open Save Folder rows.
pub(crate) fn install_system_quit_duplicate_button_hook() {
    // Do not patch the Quit Game tab's GFx component index. Runtime/user evidence shows switching
    // the native one-slot GameEnd component to the multi-slot controls component strips the native
    // character portrait/playtime/level and poisons the shared OptionSetting GFx list as soon as the
    // Quit tab is visited, even with no cloned rows selected.
    append_autoload_debug(format_args!(
        "system-quit-dup: component-index patch disabled; preserving native Quit Game GFx component"
    ));
    install_scaleform_handler_lifecycle_guard();
    // Return-to-title crash fix (er-effects-rs-j74t): the ~MenuWindowJob finalize runs its whole
    // owningMenuWindow block on a DOOMED title window during return-to-title, dereferencing wild
    // memory (crashes rva 0x7ada87 and 0x7adb28). At the destructor we reproduce the finalize's
    // vfptr[3] call and, if the window is freed/reused or its event index is out of range, null
    // owningMenuWindow so the finalize skips the block entirely.
    install_menu_window_job_dtor_guard();
    // Telemetry-only successor to the removed 5ae3965 overflow guard (dropping command lists on
    // overflow corrupts the render -- c2794d9): never alters queue behavior, only names which
    // producer's submissions grow per switch so the 0x1aeaf05 overflow can be fixed at its source.
    install_gx_cmd_queue_telemetry();
    install_system_quit_menu_window_job_run_hook();
    install_system_quit_window_list_push_hook();
    install_system_quit_save_game_text_hook();
    install_system_quit_noop_action_hook();
    install_system_quit_save_game_confirm_hook();
    install_system_quit_profile_load_activate_hook();
    install_system_quit_profile_load_confirmed_hook();
    install_system_quit_profile_load_job_run_hook();
    if SYSTEM_QUIT_DUPLICATE_INSTALLED.load(Ordering::SeqCst) != SYSTEM_QUIT_DUPLICATE_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve AddCancelButton rva 0x{SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_duplicate_add_cancel_button_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_DUPLICATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable AddCancelButton failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_DUPLICATE_INSTALLED
                        .store(SYSTEM_QUIT_DUPLICATE_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked AddCancelButton 0x{addr:x}; will clone Quit Game row as Load Profile and Load Save Profiles at caller rva 0x{SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new AddCancelButton failed: {status:?}"
        )),
    }
}

/// Install the MenuWindow-latch hook once (MinHook on the SceneObjProxy ctor 0x14074a700),
/// matching the auto-accept builder-hook precedent exactly (MhHook::new + queue_enable +
/// MH_ApplyQueued). Must run at process attach BEFORE the title builds during boot so the ctor's
/// rdx (the validated host MenuWindow*) is latched. Idempotent + harmless (latch + passthrough).
pub(crate) fn install_menu_window_latch_hook() {
    if MENU_WINDOW_LATCH_INSTALLED.load(Ordering::SeqCst) != MENU_WINDOW_LATCH_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "menuwindow-latch: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(ctor_addr) = game_rva(SCENE_OBJ_PROXY_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "menuwindow-latch: failed to resolve SceneObjProxy ctor rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            ctor_addr as *mut c_void,
            scene_obj_proxy_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCENE_OBJ_PROXY_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "menuwindow-latch: queue_enable ctor failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    MENU_WINDOW_LATCH_INSTALLED
                        .store(MENU_WINDOW_LATCH_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "menuwindow-latch: hooked SceneObjProxy ctor 0x{ctor_addr:x} (latch rdx=MenuWindow*)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "menuwindow-latch: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "menuwindow-latch: MhHook::new ctor failed: {status:?}"
        )),
    }
}

/// Install the SAVE-SAFE c30-writer diagnostic hook once (MinHook on the SOLE
/// GameMan+0xc30 writer 0x14067bd70), mirroring the MenuWindow-latch precedent exactly
/// (MH_Initialize + MhHook::new + queue_enable + MH_ApplyQueued). Installed
/// UNCONDITIONALLY at process attach. The hook (`c30_writer_hook`) is a pure
/// passthrough that forwards all args + returns the original's result; it only logs the
/// c30-write gate, c30 before/after, and a window of the resident save buffer so we can
/// diagnose why c30 stays default cold. NO SetState5, NO save write -- harmless.
pub(crate) fn install_c30_writer_hook() {
    if C30_WRITER_HOOK_INSTALLED.load(Ordering::SeqCst) != C30_WRITER_HOOK_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("c30-writer: MH_Initialize failed: {status:?}"));
            return;
        }
    }
    let Ok(writer_addr) = game_rva(C30_WRITER_RVA as u32) else {
        append_autoload_debug(format_args!("c30-writer: failed to resolve 0x67bd70 rva"));
        return;
    };
    match unsafe { MhHook::new(writer_addr as *mut c_void, c30_writer_hook as *mut c_void) } {
        Ok(hook) => {
            C30_WRITER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!("c30-writer: queue_enable failed: {status:?}"));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    C30_WRITER_HOOK_INSTALLED
                        .store(C30_WRITER_HOOK_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "c30-writer: hooked 0x{writer_addr:x} (SAVE-SAFE c30-write diagnostic; gate + c30 before/after + buffer window)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "c30-writer: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("c30-writer: MhHook::new failed: {status:?}"))
        }
    }
}

/// Clean static splash-skip patch (flip je->jg in STEP_BeginLogo) so the game's
/// own flow advances past the logo via SetState instead of playing it. Validates
/// the expected opcode first (aborts if the binary differs), and restores page
/// protection after. Spawned early at DLL attach so it lands before state 2 runs.
pub(crate) fn apply_splash_skip() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("splash-skip: module base unavailable"));
        return;
    };
    let target = (base + SPLASH_SKIP_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing != SPLASH_SKIP_EXPECTED_JE {
        append_autoload_debug(format_args!(
            "splash-skip: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{SPLASH_SKIP_EXPECTED_JE:x}",
            base + SPLASH_SKIP_RVA
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            SPLASH_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!("splash-skip: VirtualProtect failed"));
        return;
    }
    unsafe { *target = SPLASH_SKIP_REPLACEMENT_JG };
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            SPLASH_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    append_autoload_debug(format_args!(
        "splash-skip: patched 0x{:x} 0x{SPLASH_SKIP_EXPECTED_JE:x}->0x{SPLASH_SKIP_REPLACEMENT_JG:x}",
        base + SPLASH_SKIP_RVA
    ));
}

type SoundPostEventCoreFn =
    unsafe extern "system" fn(u32, u64, u32, usize, usize, *const c_void, u32) -> u32;

unsafe extern "system" fn sound_post_event_core_hook(
    event_id: u32,
    game_object: u64,
    flags: u32,
    callback: usize,
    cookie: usize,
    external_sources: *const c_void,
    event_type: u32,
) -> u32 {
    let muted = IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES;
    let ret = if muted {
        0
    } else {
        let orig = SOUND_POST_EVENT_CORE_ORIG.load(Ordering::SeqCst);
        let call: SoundPostEventCoreFn = unsafe { std::mem::transmute(orig) };
        SOUND_POST_EVENT_FORWARDED_HITS.fetch_add(1, Ordering::SeqCst);
        unsafe {
            call(
                event_id,
                game_object,
                flags,
                callback,
                cookie,
                external_sources,
                event_type,
            )
        }
    };
    let hit = SOUND_POST_EVENT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    SOUND_POST_EVENT_FIRST_ID
        .compare_exchange(0, event_id as usize, Ordering::SeqCst, Ordering::SeqCst)
        .ok();
    SOUND_POST_EVENT_LAST_ID.store(event_id as usize, Ordering::SeqCst);
    if muted {
        SOUND_POST_EVENT_MUTED_HITS.fetch_add(1, Ordering::SeqCst);
        SOUND_POST_EVENT_FIRST_MUTED_ID
            .compare_exchange(0, event_id as usize, Ordering::SeqCst, Ordering::SeqCst)
            .ok();
        SOUND_POST_EVENT_LAST_MUTED_ID.store(event_id as usize, Ordering::SeqCst);
    }
    SOUND_POST_EVENT_LAST_PLAYING_ID.store(ret as usize, Ordering::SeqCst);
    SOUND_POST_EVENT_LAST_GAME_OBJECT.store(game_object as usize, Ordering::SeqCst);
    SOUND_POST_EVENT_LAST_FLAGS.store(flags as usize, Ordering::SeqCst);
    let caller_rva = trace_first_game_caller_rva();
    SOUND_POST_EVENT_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    if hit <= 64 || hit.is_power_of_two() {
        append_autoload_debug(format_args!(
            "sound-post-event: hit={hit} muted={muted} event_id={event_id} playing_id={ret} game_obj=0x{game_object:x} flags=0x{flags:x} event_type={event_type} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

pub(crate) fn install_sound_post_event_observer_hook() {
    if SOUND_POST_EVENT_CORE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "sound-post-event: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SOUND_POST_EVENT_CORE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "sound-post-event: failed to resolve rva 0x{SOUND_POST_EVENT_CORE_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            sound_post_event_core_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SOUND_POST_EVENT_CORE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "sound-post-event: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SOUND_POST_EVENT_CORE_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "sound-post-event: hooked AK::SoundEngine::PostEvent core 0x{addr:x}; pre-world startup/title-logo Wwise events will be muted and counted"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "sound-post-event: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "sound-post-event: MhHook::new failed at 0x{addr:x}: {status:?}"
        )),
    }
}
