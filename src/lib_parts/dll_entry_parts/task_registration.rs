
pub(crate) fn spawn_game_task(state: Arc<Mutex<EffectsState>>) {
    std::thread::spawn(move || {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_THREAD_STARTED,
            BOOTSTRAP_DETAIL_START,
        );
        let cs_task = wait_for_task_instance();
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_INSTANCE_READY,
            BOOTSTRAP_DETAIL_DONE,
        );
        // Boot-phase marker: CSTaskImp resolved -> bounds the end of the pre-instance engine-init
        // gap (the largest uninstrumented boot window) in the same [+Nms] timeline the renderer parses.
        if profiler_enabled() {
            append_autoload_debug(format_args!("boot-phase: cstask_instance_ready"));
        }

        cs_task.run_recurring(
            move |task_data: &FD4TaskData| {
                // Boot-phase marker: first frame our recurring task actually ticks.
                if profiler_enabled()
                    && BOOT_FIRST_FRAME_LOGGED
                        .swap(GAME_TASK_TICK_INCREMENT as usize, Ordering::SeqCst)
                        == 0
                {
                    append_autoload_debug(format_args!("boot-phase: first_game_frame"));
                }
                // Bisect kill-switch: do nothing per frame. Isolates "our task
                // body crashes the title ~19s" from "the DLL's mere presence".
                if inert_mode() {
                    return;
                }
                // Hardware write-watchpoint on GameMan+0xc30: (re)arm each frame until
                // the save-mount write is caught, so the VEH logs the exact writer. Runs
                // HARD input block (DInput keyboard+mouse + XInput gamepad), driven from the
                // game task so it is active even when no render callback is running
                // (it does not under the offline launcher at the title). Runs every frame the
                // task ticks -- before the player check -- so a focused window cannot inject any
                // real input during the zero-input own-stepper/autoload probe. Pure suppression,
                // never synthesis.
                if block_input_enabled() {
                    enforce_input_block_now();
                } else {
                    release_input_block_now();
                }
                // GameMan field transition trace (change-detected): captures the STABLE boot-load
                // trajectory and the BOUNCE switch-load trajectory in one run so they can be diffed to
                // find which GameMan field re-triggers the title post-load. Runs every frame; the
                // change-detection makes it a compact transition log. Product-autoload runs only.
                if product_autoload_enabled() {
                    snapshot_game_man_on_change();
                }
                // SELF-DRIVEN System->Quit->Load-Profile repro autopilot: stamps this frame's
                // scripted DInput key (no-op unless system_quit_repro_enabled + in-world). Runs
                // every frame so the injected key is fresh for the game's keyboard poll, and only
                // while the block above is engaged (which the autopilot itself keeps on in-world).
                unsafe { system_quit_repro_tick() };
                // D3D12 PRESENT OVERLAY: once the GX device is up, find the game's live swapchain and hook
                // its REAL Present (the dummy-swapchain vtable differs under vkd3d-proton). Self-gated
                // (portrait path only, one-shot on success, bounded retries) so it's cheap every frame.
                if let Ok(base) = game_module_base() {
                    unsafe { try_install_game_present_hook(base) };
                }
                // LOADING-COVER EXPERIMENT: clear CSFakeLoadingScreenImp.visible each frame so the world
                // draws uncovered during map loads. Self-gates (disable_loading_cover_enabled); runs before
                // the player check so it acts during the loading screen (player absent). catch_unwind so a
                // torn cover pointer can never fault the game thread.
                if let Ok(base) = game_module_base() {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                        suppress_loading_cover_tick(base)
                    }));
                }
                // before the player check so it arms at the title (pre-load), independent
                // of the active observe/own-stepper mode.
                if c30_watch_enabled() {
                    if let Ok(base) = game_module_base() {
                        let frame = C30_WATCH_FRAME_COUNTER
                            .fetch_add(C30_WATCH_HIT_INCREMENT, Ordering::SeqCst)
                            as u64;
                        unsafe { maybe_arm_c30_watch(base, frame) };
                    }
                }
                // RECURRING world-stream observer (own-load-stream-observer-must-be-recurring-task-2026-06-22).
                // Internally no-ops until own_load_continue_fire sets OWN_LOAD_CONTINUE_FIRED, so it
                // costs nothing during normal play and never spams. After continue_confirm/SetState5
                // fires, own_stepper_idx10 (a TITLE-PHASE task) STOPS ticking, so this per-frame game
                // task is the ONLY place that keeps logging the world-stream pump THROUGH the loading
                // screen. Runs BEFORE the player check so it ticks while there is no player yet (the
                // loading-screen frames are exactly when player_present is false). Pure reads only.
                // GOLDEN baseline mode (golden_observe_enabled) ALSO drives the observer even though our
                // continue never fired, so a NORMAL user-driven vanilla load is captured for diffing
                // against the menu-free OWN-LOAD stall. The observer self-gates and re-resolves the
                // owner->InGameStep->MoveMapStep chain live from OWN_LOAD_OWNER_CACHED (filled by
                // own_stepper_idx10 each title frame in golden mode). OBSERVE-ONLY: no load is fired.
                // OBSERVE-ONLY WorldBlockRes::Update diagnostic detour (worldblockres-phase-machine-
                // drives-loadstate-to-0xa-2026-06-22): installed ONCE (idempotent) whenever a diagnostic
                // OWN-LOAD / golden-observe context is armed, so normal play is untouched. The detour is a
                // pure-read pass-through (bumps a call counter + tracks max phase/gate atomics, then calls
                // the original and returns its value), so installing early is harmless and never alters
                // load behavior. It answers: is WorldBlockRes::Update ticked at all on our path, and do
                // any blocks' phase ([+0x35]) / FD4 gate ([+0x2f]) advance.
                if own_load_enabled()
                    || own_load_continue_enabled()
                    || own_load_pump_enabled()
                    || golden_observe_enabled()
                {
                    install_wbr_update_hook();
                }
                if (own_load_enabled() && OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst))
                    || golden_observe_enabled()
                {
                    if let Ok(base) = game_module_base() {
                        let gm = game_man_ptr_or_null();
                        let player_present = unsafe { PlayerIns::local_player_mut() }.is_ok();
                        unsafe { own_load_stream_observe_recurring(base, gm, player_present) };
                    }
                }
                // PATH B PRIVATE PUMP (own_load_pump): if own_load_pump_fire built+armed the LoadGame job,
                // tick its Run privately EVERY frame here (the game thread) -- replicating native
                // ExecuteMenuJob's call shape (zero-init MenuJobResult + FD4Time carrying the frame delta)
                // -- to drive self-build -> deser -> m28 stream, then SetState5 on Success. Self-gates on
                // OWN_LOAD_PUMP_JOB != 0 / OWN_LOAD_PUMP_DONE, so it costs nothing until armed+built and
                // never re-pumps once terminal. Must run THROUGH the loading screen (player absent), so it
                // is here in the recurring game task, before the player check. Pure native call + reads.
                if own_load_pump_enabled() {
                    if let Ok(base) = game_module_base() {
                        let gm = game_man_ptr_or_null();
                        let frame_delta = task_data.delta_time.time;
                        unsafe { own_load_pump_tick(base, gm, frame_delta) };
                    }
                }
                // DIRECT "Continue pressed" trigger: at the settled main menu (post press-any-button,
                // GameMan set up), write the exact bit the native selector consumes
                // (*(TitleFlowContext+0x14c)=1), invoke the selector to BUILD the LoadGame job, and
                // PushBackJob it to the dialog queue. Self-gates + fires once; no input. Then DRAIN the
                // queue each frame (FUN_1407a90f0) so the posted job runs to completion (deser+world).
                if fire_tfc_continue_enabled() {
                    if let Ok(base) = game_module_base() {
                        // Autonomous press-any-button: self-fire the open-menu registrar when the
                        // title settles (zero-input), so no real button press is needed.
                        unsafe { maybe_auto_open_menu(base) };
                        // The Continue BUILD now runs IN-CONTEXT from the hooked TitleTopDialog::update
                        // detour (the pump's live-dialog frame), NOT from this game task -- that timing
                        // was the mis-context cause. Install the hook once; the detour fires the build.
                        unsafe { install_title_update_hook(base) };
                        let frame_delta = task_data.delta_time.time;
                        unsafe { tfc_continue_drain_tick(base, frame_delta) };
                    }
                }
                // GOLDEN-PATH zero-input boot -> open menu (DECOUPLED from fire_tfc_continue): the
                // readiness-gated press-any-button advance (hook 0x1407ad1c0 -> set [job+0x1e8]=2)
                // gets PAST press-any-button with no input, then the menu opens with NO selector fire,
                // so an observe run can reach the menu cleanly. bd
                // press-any-button-golden-lever-job1e8-readiness-2026-06-23.
                //
                // The menu OPEN is driven the NATIVE way: set the decoded global accept byte
                // 0x144589bdc=1 once at the settled title so the game's OWN TitleTopDialog::update
                // accept-gate runs the open-menu registrar in its native frame -- which POSTS the
                // Continue/Load/NewGame MenuJob chain AND drains it (MenuWindow::Update) in the same
                // flow, so the rows actually build. A direct registrar self-fire (maybe_auto_open_menu)
                // only POSTED the chain; the native update does not drain a chain it did not open, so
                // the rows never built (continue-scan = 0 nodes, stage 3). Zero-input (decoded accept
                // flag, not a synthesized event). bd er-effects-rs-e9e + rowbuild-mechanism-incontext-
                // openmenu-2026-06-23.
                if pab_advance_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_pab_advance_hook(base) };
                        if !native_profile_capture_enabled() {
                            unsafe { maybe_set_title_accept_byte(base) };
                        }
                    }
                }
                // Now-loading helper observer: attach only after the native title accept byte fired.
                // Attach-time detours on CSNowLoadingHelperImp exited before readiness; this delayed
                // install avoids touching the loading helper until the title path has already advanced.
                if product_autoload_enabled()
                    && TITLE_ACCEPT_BYTE_GATE_FIRED.load(Ordering::SeqCst)
                    && NOW_LOADING_HELPER_HOOKS_INSTALLED.load(Ordering::SeqCst) == 0
                {
                    install_now_loading_helper_observer_hooks();
                }
                // Title transition fast-forward (pab_dismiss -> menu_open): scale the title
                // frame-delta so the FadeIn/TextFadeOut/menu Scaleform animation reaches its end
                // frame in fewer wall-clock frames. Default-on product behavior for real runs (the
                // detour self-gates per frame); install once. bd er-effects-rs-urw.
                if title_anim_speedup_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_title_anim_speed_hook(base) };
                        // READ-ONLY native state-transition timeline (menu-build-overlap lever
                        // "look before acting" instrument): logs every SetState(owner,int) with a
                        // timestamp so we learn exactly when BeginTitle(3) fires and whether the
                        // 05_000_Title build has headroom to start earlier. Save-safe pass-through.
                        unsafe { install_title_setstate_trace_hook(base) };
                    }
                }
                // OFFLINE connection-state lever (milestone-3 fix): force GameMan+0xBC8/0xBC9 = 0 each
                // title frame so the connection-loss event handlers -- which build the GR_System_Message
                // "Cannot connect to network / connection lost" MessageBoxDialogs our offline boot
                // raises at menu-open -- short-circuit at their `IsInOnlineMode() &&
                // IsServerConnectionEnabled()` guard before enqueuing any popup. Gated by the offline
                // flag (this only forces state the offline boot already intends). bd er-effects-rs-0ye.
                if online_disable_enabled() {
                    // MILESTONE-3 FIX: short-circuit the offline title-flow check jobs to their
                    // no-modal exits so the title flow never enqueues a GR_System_Message MessageBox.
                    // ShowProgressJob::Run is the shared chokepoint for the save/network/sign-in/login
                    // check steps (the 3 observed modals); NetworkCheckJob::Run is the separate J6 job.
                    // Installed once, before menu-open. Offline-gated (no effect on an online check).
                    install_network_check_shortcircuit_hook();
                    install_show_progress_shortcircuit_hook();
                    if let Ok(base) = game_module_base() {
                        unsafe { force_offline_connection_bytes(base) };
                    }
                }
                // DIAGNOSTIC (gated by er-effects-grsysmsg-log.txt): log the GR_System_Message ids the
                // title flow fetches after menu-open, to DEFINITIVELY name the menu-open MessageBoxDialogs
                // (connection 4101/4102/4190 vs save 70000/4191) instead of guessing. Self-gates once.
                // Also install whenever a save load is expected (not telemetry-only / not trace):
                // the same GetGR_System_Message hook carries the corrupted-save SEMAPHORE
                // (oracle_corrupted_save_seen_id), so a load probe records the "save data is corrupted"
                // popup as RAM-read telemetry instead of a one-off on-screen image.
                if grsysmsg_log_enabled()
                    || (!save_override_telemetry_only() && !save_trace_enabled())
                {
                    install_gr_sysmsg_log_hook();
                }
                // Anti-anti-debug (ported from ProDebug, correct base): neutralize FromSoft's
                // timed anti-debug so debug exceptions / our INT3 breakpoints reach our VEH.
                // Runs ONCE, BEFORE arming breakpoints, from the game task (game up, .text
                // decrypted) -- our own controlled timing, not the LazyLoader's.
                if anti_antidebug_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { apply_anti_antidebug_once(base) };
                    }
                }
                // Software (INT3) breakpoints from er-effects-breakpoints.txt: install once.
                // The VEH (crash logger) logs every hit's register/stack context + re-arms.
                if sw_breakpoints_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_sw_breakpoints_once(base) };
                    }
                }
                // STAY-ACTIVE: force ER's input-accept flag so a virtual gamepad keeps driving the
                // menus while ER is UNFOCUSED (user can work elsewhere during a golden capture). ER
                // clears [DLUID+0x88d] each frame when it isn't GetActiveWindow; re-set it to 1.
                if stay_active_enabled() {
                    if let Ok(base) = game_module_base() {
                        // DLUID (input-device-manager) singleton VA 0x14485dc18.
                        const DLUID_SINGLETON_RVA: usize =
                            RuntimeGlobalRva::DluidInputManager as usize;
                        #[repr(C)]
                        struct DluidInputManagerLayout {
                            unknown_000: [u8; 0x88d],
                            input_active: u8,
                        }
                        const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize =
                            core::mem::offset_of!(DluidInputManagerLayout, input_active);
                        const INPUT_ACTIVE: u8 = true as u8;
                        const NULL_DLUID: usize = NULL_MODULE_BASE;
                        let dluid = unsafe { safe_read_usize(base + DLUID_SINGLETON_RVA) }
                            .unwrap_or(NULL_DLUID);
                        // Defensive: only write once the flag byte is confirmed READABLE (so a
                        // not-yet-initialized or bad singleton ptr can never fault the game thread).
                        if dluid != NULL_DLUID
                            && unsafe { safe_read_usize(dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) }
                                .is_some()
                        {
                            unsafe {
                                *((dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) as *mut u8) =
                                    INPUT_ACTIVE
                            };
                        }
                    }
                }
                let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
                    let mut state = state_or_return(&state);
                    state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                    // Install the MessageBoxDialog builder hook for native telemetry. Product
                    // autoload must NOT auto-accept: every pre/post-load message box is a hard
                    // investigation trigger whose semantic side effect must be skipped directly.
                    // The legacy OK-handler dismiss path remains only for non-product probes.
                    if online_disable_enabled() {
                        install_auto_accept_hook();
                        if !product_autoload_enabled() {
                            force_dismiss_startup_dialog();
                        }
                    }
                    // Observe the natural flow PAST the modal: tap Confirm (game's own input).
                    if auto_confirm_enabled() {
                        auto_confirm_tap();
                    }
                    // Bisect kill-switch: lock + tick only, NO filesystem I/O
                    // (no telemetry write, no experiments). Discriminates "our
                    // per-frame file I/O stalls the title" (lite survives) from
                    // "any per-frame work trips a budget" (lite still exits).
                    if lite_mode() {
                        return;
                    }
                    unsafe { system_quit_profile_select_top_menu_tick() };
                    // Product autoload: run the native title open-menu predicate + minimal
                    // native save-load core from the recurring game task, before the idx10
                    // MenuJobWait hook path is needed. This bypasses title-accept/input
                    // injection while still advancing the data-driven PressStart/PRESS BUTTON
                    // component through its native open-menu registrar; readiness is checked
                    // inside product_core_autoload_tick.
                    if product_autoload_enabled() {
                        PRODUCT_CORE_CALLSITE_TICKS.fetch_add(1, Ordering::SeqCst);
                        let base_result = game_module_base();
                        if base_result.is_ok() {
                            PRODUCT_CORE_CALLSITE_BASE_OK_TICKS.fetch_add(1, Ordering::SeqCst);
                        }
                        let quickload_slot =
                            SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
                        let slot_result = if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                            >= SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
                            && quickload_slot != usize::MAX
                        {
                            Some(quickload_slot as i32)
                        } else {
                            state.autoload.slot()
                        };
                        if let Some(slot) = slot_result {
                            PRODUCT_CORE_CALLSITE_SLOT_OK_TICKS.fetch_add(1, Ordering::SeqCst);
                            PRODUCT_CORE_CALLSITE_LAST_SLOT.store(slot as usize, Ordering::SeqCst);
                        }
                        if let (Ok(base), Some(slot)) = (base_result, slot_result) {
                            unsafe {
                                product_core_autoload_tick(base, slot, state.game_task_ticks)
                            };
                            // FIRST-CHARACTER PORTRAIT BAKE YOINKED (user 2026-07-03). This one-shot
                            // (LOADING_BG_PORTRAIT_GX_KEPT, set once) captured the BOOT autoload
                            // target's portrait CSGxTexture and baked it into the now-loading forge --
                            // the reason the FIRST character (and only the first) had its portrait
                            // baked into the loading screen, distinct from the per-frame overlay path
                            // the System->Quit switch characters use. Suppressing just this leaves the
                            // switch portraits untouched. (The forge/checker + loading-art coupling is
                            // a separate decouple, tracked for later.) The capture fn + its title.rs
                            // (default-off flow) caller remain for reference.
                            let _ = maybe_capture_portrait_gxtexture;
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // FORCE LIVE PROFILE PORTRAIT RENDER (diagnostic, default-OFF): while the user
                    // holds the ProfileSelect/Load-Game screen (valid menu render context, NO
                    // Continue commit), mark the target slot used + kick the async character-model
                    // build so the renderer renders the live 3D head into its offscreen. Menu-phase
                    // only -> no Continue/teardown/world-load crash path. The capture keeps the gx
                    // once the model latches (+0x778). Validates P1 (the build) in isolation.
                    if force_profile_render_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe {
                                force_profile_render_tick(base, FORCE_PROFILE_RENDER_MANUAL_SLOT)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // OWN-THE-STEPPER: patch the idx10 step-fn slot to our handler so
                    // the FD4 scheduler runs OUR code in-context (step 1: verify the
                    // control point with a logging pass-through).
                    // OWN-STEPPER and the SEPARATE observe-only NATIVE-LOAD gate both install the
                    // idx10 patch so OUR handler runs each frame. own_stepper_idx10 dispatches to
                    // the native-load (observe-only, no forcing) path when native_load_enabled().
                    if own_stepper_enabled()
                        || native_load_enabled()
                        || native_continue_enabled()
                        || native_fullread_enabled()
                        || own_load_enabled()
                    {
                        if let Ok(base) = game_module_base() {
                            unsafe { own_stepper_patch_once(base) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Pure observe: log the title->menu->load transition each interval
                    // with NO forcing, to capture what the REAL button press does.
                    if observe_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { title_observe_tick(base, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Read-only: log the native autoload-arm preconditions
                    // (especially [slotmgr+0x8]) to decide the zero-input path.
                    if arm_probe_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { arm_precondition_probe(base, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Lever 2: zero-input title-accept via input-event injection
                    // (staged probe -> fill -> inject) to bootstrap the front-end.
                    if title_accept_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe {
                                title_accept_tick(
                                    base,
                                    state.game_task_ticks,
                                    title_accept_inject_enabled(),
                                )
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Corrected play-game submit: on the live FE-host at state 10,
                    // SetState(5) with a packed map (not raw state/slot like the old
                    // force_play_game) so the existing pump builds CSFeMan + loads.
                    if submit_play_game_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe {
                                submit_play_game_once(base, slot, state.game_task_ticks, task_data)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Per-frame native arm: re-set the slot each frame + latch so
                    // the save-mgr update can arm before the title resets the slot.
                    if native_arm_loop_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe { native_arm_loop_tick(base, slot, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Recipe Option 1 (flagless): drive the genuine offline
                    // continue (MoveMapList dispatcher + b73) to load the REAL slot.
                    if continue_drive_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe { continue_drive_tick(base, slot, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Recipe B (flagless): drive the outer IngameInit once + pump
                    // the InGameStep. Self-contained -- skips the other autoload
                    // branches to avoid double-submit. Needs the live FD4TaskData.
                    if ingameinit_drive_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe {
                                ingameinit_drive_tick(base, slot, state.game_task_ticks, task_data)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    process_safe_input_request(&mut state);
                    process_autoload_request(&mut state);
                    // Direct-drive the orphaned InGameStep load once force_play_game
                    // has reached GameStepWait (run 305: hooking the step pump froze
                    // the title, so we call its Execute directly with the live ctx).
                    if ingamestep_pump_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { ingamestep_pump_tick(base, task_data) };
                        }
                    }
                    write_telemetry_throttled(&mut state, false);
                    return;
                };

                let mut state = state_or_return(&state);
                state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                // In-world: latch OFF the startup popup auto-accept (in-game dialogs need real
                // choices), optionally clean stale title-dialog render resources, then run the
                // one-shot correctness dump.
                IN_WORLD_REACHED.store(IN_WORLD_REACHED_YES, Ordering::SeqCst);
                if product_autoload_enabled()
                    && SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
                {
                    PRODUCT_CORE_CALLSITE_TICKS.fetch_add(1, Ordering::SeqCst);
                    let quickload_slot = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
                    if let (Ok(base), true) = (game_module_base(), quickload_slot != usize::MAX) {
                        PRODUCT_CORE_CALLSITE_BASE_OK_TICKS.fetch_add(1, Ordering::SeqCst);
                        PRODUCT_CORE_CALLSITE_SLOT_OK_TICKS.fetch_add(1, Ordering::SeqCst);
                        PRODUCT_CORE_CALLSITE_LAST_SLOT.store(quickload_slot, Ordering::SeqCst);
                        unsafe {
                            product_core_autoload_tick(
                                base,
                                quickload_slot as i32,
                                state.game_task_ticks,
                            )
                        };
                    }
                    write_telemetry_throttled(&mut state, false);
                    return;
                }
                if own_stepper_enabled()
                    || native_load_enabled()
                    || native_continue_enabled()
                    || native_fullread_enabled()
                {
                    if let Ok(base) = game_module_base() {
                        unsafe {
                            cleanup_title_dialog_after_world_once(base, state.game_task_ticks)
                        };
                    }
                }
                // In-world correctness oracle: on the FIRST frame the local player exists, log
                // the load-correctness record + the T_controllable timeline marker ONCE. Fires
                // for both a native-menu load (observe) and a DLL-driven load (own-stepper), so
                // the two records are directly comparable (field-for-field == correct load).
                if (own_stepper_enabled()
                    || observe_enabled()
                    || native_load_enabled()
                    || native_continue_enabled()
                    || native_fullread_enabled())
                    && LOAD_CORRECTNESS_DUMPED
                        .swap(GAME_TASK_TICK_INCREMENT as usize, Ordering::SeqCst)
                        == LOAD_CORRECTNESS_NOT_DUMPED
                {
                    if let Ok(base) = game_module_base() {
                        timeline_event(
                            "T_controllable",
                            state.game_task_ticks,
                            format_args!("player=1"),
                        );
                        unsafe { dump_load_correctness(base, state.game_task_ticks) };
                    }
                }
                let observation = observe_animation(player, state.last_write_idx);
                state.current_animation_id = observation.current_animation_id;
                if observation.current_animation_id == Some(APPEAR_ANIMATION_ID)
                    || observation.appear_newly_queued
                {
                    state.expected_animation_seen = true;
                }
                state.last_write_idx = Some(observation.write_idx);

                remove_requested_calls(player, &mut state);
                process_driver_command(player, &mut state);

                let appear_playing = observation.current_animation_id == Some(APPEAR_ANIMATION_ID);
                if !appear_playing {
                    state.applied_for_current_appear = false;
                }

                let should_apply_for_appear = (observation.appear_newly_queued || appear_playing)
                    && !state.applied_for_current_appear;
                let should_apply = should_apply_for_appear || state.manual_apply_requested;
                state.manual_apply_requested = false;

                if should_apply_for_appear {
                    state.applied_for_current_appear = true;
                }

                if should_apply {
                    apply_selected_calls(player, &mut state);
                }

                process_global_driver_command(&mut state);
                refresh_call_status(player, &mut state);
                write_telemetry_throttled(&mut state, true);
            },
            CSTaskGroupIndex::FrameBegin,
        );
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_RECURRING_REGISTERED,
            BOOTSTRAP_DETAIL_DONE,
        );
        // REALTIME PORTRAIT LOOK-AT draw-phase SWEEP: register the realtime draw task in EACH candidate
        // DRAW phase, so it runs on the render thread inside an actively-recording GX frame (where the
        // profile draw step's GX subcontext-pool pop succeeds -- FrameBegin, above, is before the frame
        // records, so a draw there is a black no-op). Each registration bumps its own per-frame tick
        // counter; only the phase whose index == PROFILE_LOOKAT_SELECTED_PHASE actually rasterizes, so
        // exactly one phase draws per frame. The active phase is switchable live via
        // er-effects-lookat-phase.txt (no recompile), to find one that ticks per-frame at the menu
        // (GameSceneDraw measured ~11% -- world-gated). We own these tasks (cancel() is a fromsoftware-rs
        // no-op + self-leaked Arc), so the chosen one persists past Continue = the loading-screen port.
        // Order MUST match constants::LOOKAT_DRAW_PHASE_NAMES.
        let lookat_phases = [
            CSTaskGroupIndex::Draw_Pre,
            CSTaskGroupIndex::GraphicsStep,
            CSTaskGroupIndex::DrawStep,
            CSTaskGroupIndex::DrawBegin,
            CSTaskGroupIndex::GameSceneDraw,
            CSTaskGroupIndex::AdhocDraw,
            CSTaskGroupIndex::DrawEnd,
            CSTaskGroupIndex::Draw_Post,
        ];
        for (i, phase) in lookat_phases.into_iter().enumerate() {
            cs_task.run_recurring(
                move |task_data: &FD4TaskData| unsafe {
                    profile_lookat_phase_draw_tick(i, task_data)
                },
                phase,
            );
        }
        // Sweep diagnostic + live selector re-read, paced by a FrameBegin task (ticks every frame).
        cs_task.run_recurring(
            move |_task_data: &FD4TaskData| profile_lookat_phase_diag_tick(),
            CSTaskGroupIndex::FrameBegin,
        );
        // BUILD-OWN LIVE-RENDER DRIVER (gated, FrameBegin = GAME thread, ticks EVERY frame incl. the
        // loading screen). force_profile_render_tick's only other call sites are menu-phase-only (they
        // `return` before Continue), so maybe_build_profile_table_for_loading + the mark/refresh feed never
        // ran post-Continue -> loadbuilds=0, the loaded character never re-built. Driving it here gives the
        // build-own path a post-Continue game-thread driver: it builds our OWN profile renderers (engine
        // 10-slot builder), which self-register their ResMan model build/draw tasks and OWN their model with
        // OUR lifetime (no teardown-free -> no AV, unlike re-attaching the dying menu model). The fn
        // self-gates heavily (table-ready, feature gates, one-shots), so an every-frame call is idempotent.
        // Gated by portrait_render_drive_enabled so it can be A/B'd against the safe checker baseline.
        cs_task.run_recurring(
            move |_task_data: &FD4TaskData| {
                if let Ok(base) = game_module_base() {
                    // Stats-panel neutral-bg register: runs on EVERY frame regardless of the autoload
                    // path (the `save_requested` product path never enters product_core_autoload_tick,
                    // so the register cannot live there). Self-gating (stats_panel_enabled + repos-ready
                    // + idempotent per slot via the registered mask), so an every-frame call is cheap
                    // and stops attempting once all 10 slots are registered.
                    unsafe { maybe_register_stats_panel_textures(base) };
                    if portrait_render_drive_enabled() {
                        unsafe {
                            force_profile_render_tick(base, FORCE_PROFILE_RENDER_MANUAL_SLOT)
                        };
                    }
                }
            },
            CSTaskGroupIndex::FrameBegin,
        );
    });
}
