use super::*;

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
    // owningMenuWindow block on a doomed title window during return-to-title, dereferencing wild
    // memory (crashes rva 0x7ada87 and 0x7adb28). At the destructor we reproduce the finalize's
    // vfptr[3] call and, if the window is freed/reused or its event index is out of range, null
    // owningMenuWindow so the finalize skips the block entirely.
    install_menu_window_job_dtor_guard();
    // The dtor guard covers only the finalize's 0x7ac720 caller; the switch crash arrives via
    // MenuWindowJob::Run. Hook the finalize itself so every caller is covered.
    install_menu_window_job_finalize_guard();
    // The three traces that used to be installed here are gone (2026-08-25). `install_msb_parse_trace`,
    // `install_loadlist_wait_trace` and `install_dlc_roots_trace` sat on these three lines with no
    // gate above them, so a shipped profile detoured the sole `msbResCap` writer (once per MSB, every
    // boot), `STEP_LoadListWait` (every frame) and the three DLC virtual-root entry points -- five
    // detours whose entire output was a log line, and whose results no `oracle_*` field ever read
    // back. They moved verbatim into `crates/er-diag-harness/`, a separate `[[natives]]` shell that
    // an agent adds to a profile when it wants them. Nothing else in the product called them.
    //
    // Quit-to-desktop clean kill: on a quit the world teardown unloads the MenuOffscrRendParam param
    // table and the rebuilt title's model renderer DLPanics on the missing table. Turn that exact
    // condition into a fast clean ExitProcess(0) (save-then-kill) instead of the crash.
    install_quit_to_desktop_clean_kill_hook();
    // Telemetry-only successor to the removed 5ae3965 overflow guard (dropping command lists on
    // overflow corrupts the render -- c2794d9): never alters queue behavior, only names which
    // producer's submissions grow per switch so the 0x1aeaf05 overflow can be fixed at its source.
    install_gx_cmd_queue_telemetry();
    // Disabled 2026-07-15: this detour targets 0x7ad1c0, the same RVA as the default-on PAB detour
    // (PAB_NODE_UPDATE_RVA == MENU_WINDOW_JOB_RUN_RVA). MinHook binds only one detour per address, and on
    // native Windows the inline/early PAB install always wins, so this background-thread install fails
    // ALREADY_CREATED and its post-original work never ran (ghosting + non-interactive ProfileSelect). Its
    // post-original body (system_quit_menu_window_run_post) is now called directly from the guaranteed
    // winner, pab_node_update_detour, so this contender is removed to keep PAB the deterministic sole owner
    // (otherwise a rare System->Quit win would starve pab_advance_try, the autoload driver). See that detour.
    // install_system_quit_menu_window_job_run_hook();
    #[cfg(feature = "quit-rows")]
    install_system_quit_window_list_push_hook();
    install_system_quit_save_game_text_hook();
    // The three routing detours this used to install are part of the shared arm call below
    // (`er_quit_menu_core::row_cloner::arm`), on the `er-hook` union rather than a bare `MhHook`.
    install_system_quit_save_game_confirm_hook();
    // Save-flow confirm boxes: observe `CS::MenuJob::EmitResult` so the user's Yes/No on a
    // Save Game confirm is read from the game's own `MenuJobResult` instead of guessed from
    // dialog fields (the 2026-07-28 defect where a fresh box resolved itself to No).
    install_menu_job_emit_result_hook();
    // The three ProfileSelect routing detours exist to carry a cloned row's press into the
    // switch. Without the rows there is no press to carry.
    #[cfg(feature = "quit-rows")]
    install_system_quit_profile_load_activate_hook();
    #[cfg(feature = "quit-rows")]
    install_system_quit_profile_load_confirmed_hook();
    #[cfg(feature = "quit-rows")]
    install_system_quit_profile_load_job_run_hook();
    // Save-picker browse-row integrity (er-effects-rs-xlqh): re-stage the picker's browse rows at
    // the entry of the native ProfileSelect list builder, so an in-world game save that rewrote the
    // active slot's ProfileSummary record (MarkProfileIndexAsUsed + FUN_140262270 stomping the
    // loaded character's name over a staged row) can never leak a stray character-name row into the
    // browse list.
    // The picker itself lives in `er-quit-menu-core` since 2026-09-11. These are the steps only a
    // host with a save-swap ledger, a save flow and a live-layout editor behind it can perform; a
    // standalone shell installs none of them and the picker still browses and picks.
    super::super::save_picker::save_picker_menu::install_product_save_picker_hooks();
    install_save_picker_list_builder_hook();
    // Everything below clones rows onto the Quit tab. With the feature off the tab keeps exactly
    // what the game ships, and the installs above -- the telemetry, the vanilla Save Game hooks
    // and the picker -- still run.
    #[cfg(feature = "quit-rows")]
    {
        if SYSTEM_QUIT_DUPLICATE_INSTALLED.load(Ordering::SeqCst)
            != SYSTEM_QUIT_DUPLICATE_NOT_INSTALLED
        {
            return;
        }
        // One arm call, shared with the standalone `er-quit-menu` shell (2026-09-11). The cloner, the
        // row router and the three routing detours all moved to `er_quit_menu_core::row_cloner`, so the
        // product and a shell install the same code rather than two implementations of it -- which is
        // what makes the shell's path testable by running the product. Every detour goes through the
        // `er-hook` union: `AddCancelButton` takes five arguments, and until `er_hook::UnionFn5`
        // existed this prologue was the one row-building hook holding MinHook's single slot by itself.
        //
        // The product arms every row and supplies the four flows this crate does not own. A shell arms
        // `RowSet::BUILD_ROWS_ONLY` and supplies none, so a row whose flow it lacks is never on the tab.
        let armed = unsafe {
            er_quit_menu_core::row_cloner::arm(
                er_quit_menu_core::row_cloner::RowSet::ALL,
                er_quit_menu_core::row_cloner::QuitRowActions {
                    open_profile_load_dialog: Some(system_quit_open_profile_load_dialog),
                    open_save_picker_menu: Some(open_save_picker_menu_for_row),
                    save_game_start_flow: Some(
                        crate::experiments::system_quit_save_game_start_flow,
                    ),
                    save_game_request_save_only: Some(
                        crate::experiments::system_quit_save_game_request_save_only,
                    ),
                    // No product half: the moved reset already clears the row table, the link
                    // field and the export latch, which is everything this side used to do.
                    row_table_reset: None,
                    note_drive_strip_click_event: Some(save_picker_note_drive_strip_click_event),
                },
            )
        };
        match armed {
            Ok(()) => SYSTEM_QUIT_DUPLICATE_INSTALLED
                .store(SYSTEM_QUIT_DUPLICATE_INSTALLED_YES, Ordering::SeqCst),
            // The flag is only raised on success, so a failure leaves it at
            // `SYSTEM_QUIT_DUPLICATE_NOT_INSTALLED` and a later call retries.
            Err(error) => append_autoload_debug(format_args!(
                "system-quit-dup: arming the Quit rows failed: {error:?} -- no rows will be cloned"
            )),
        }
    }
}

#[cfg(feature = "quit-rows")]
/// The row router's save-picker arm, adapted to the action table's shape.
///
/// # Safety
///
/// Menu thread, with the row's action object.
unsafe fn open_save_picker_menu_for_row(action_obj: usize) -> bool {
    matches!(
        unsafe { system_quit_open_save_picker_menu(action_obj) },
        er_save_picker_core::PickerOpenOutcome::Opened
    )
}

/// Install the MenuWindow-latch hook once (MinHook on the SceneObjProxy ctor 0x14074a700),
/// matching the auto-accept builder-hook precedent exactly (MhHook::new + queue_enable +
/// MH_ApplyQueued). Must run at process attach before the title builds during boot so the ctor's
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
    let Ok(ctor_addr) = game_rva_for_hook(SCENE_OBJ_PROXY_CTOR_RVA) else {
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
                    crate::mh::leak_installed_hook(hook);
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

/// Clean static splash-skip patch (flip je->jg in STEP_BeginLogo) so the game's
/// own flow advances past the logo via SetState instead of playing it. Validates
/// the expected opcode first (aborts if the binary differs), and restores page
/// protection after. Spawned early at DLL attach so it lands before state 2 runs.
pub(crate) fn apply_splash_skip() {
    let Some(address) = er_title_flow::splash_skip_je_address() else {
        append_autoload_debug(format_args!(
            "splash-skip: STEP_BeginLogo has no verified address for this build -- not patching"
        ));
        return;
    };
    let target = address as *mut u8;
    let existing = unsafe { *target };
    if existing != SPLASH_SKIP_EXPECTED_JE {
        append_autoload_debug(format_args!(
            "splash-skip: ABORT -- byte at 0x{address:x} is 0x{existing:x}, expected 0x{SPLASH_SKIP_EXPECTED_JE:x}"
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
        "splash-skip: patched 0x{address:x} 0x{SPLASH_SKIP_EXPECTED_JE:x}->0x{SPLASH_SKIP_REPLACEMENT_JG:x}"
    ));
}

pub(crate) type SoundPostEventCoreFn =
    unsafe extern "system" fn(u32, u64, u32, usize, usize, *const c_void, u32) -> u32;

pub(crate) unsafe extern "system" fn sound_post_event_core_hook(
    event_id: u32,
    game_object: u64,
    flags: u32,
    callback: usize,
    cookie: usize,
    external_sources: *const c_void,
    event_type: u32,
) -> u32 {
    let in_world_seen = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
    let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let quickload_active = quickload_phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE;
    let player_present = if in_world_seen {
        unsafe { PlayerIns::local_player_mut() }.is_ok()
    } else {
        false
    };
    let muted = !in_world_seen || quickload_active || !player_present;
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
            "sound-post-event: hit={hit} muted={muted} event_id={event_id} playing_id={ret} game_obj=0x{game_object:x} flags=0x{flags:x} event_type={event_type} in_world_seen={in_world_seen} player_present={player_present} quickload_phase={quickload_phase} caller_rva=0x{caller_rva:x}"
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
    let Ok(addr) = game_rva_for_hook(SOUND_POST_EVENT_CORE_RVA as u32) else {
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
                    crate::mh::leak_installed_hook(hook);
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
