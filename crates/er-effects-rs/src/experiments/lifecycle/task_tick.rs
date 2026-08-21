//! Per-frame experiment and product lifecycle orchestration.

use super::*;
use er_title_flow::dlc_roots_self_heal_tick;

pub(crate) fn tick_before_player_lookup(task_data: &FD4TaskData) {
    // PROFILE-SWITCH RELOAD SOFTLOCK FIX (bd
    // FORK-RESOLVED-refill-job-never-enqueued-on-reload-fix-is-gated-self-heal-2026-07-30): the
    // title start-game flow blanks the 13 `*_dlc2` DLIO virtual roots on every pass, but the job
    // that repopulates them is enqueued only on boot -- so a reload leaves `mapstudio_dlc2` empty,
    // every `m28` msb read returns 0 bytes, and WorldBlockRes case 2 waits forever with no timeout.
    // Restores the root by calling the game's OWN refill, gated on the populated -> empty edge and
    // verified against the expected string. Runs before the player lookup because the damage is done
    // during the load, long before a player exists.
    unsafe { dlc_roots_self_heal_tick() };
    // LOAD2 WORLD-COMPLETION (bd load2-sole-failing-gate-is-shouldsave-save_requested-b72): when a
    // committed reload parks at MoveMapStep finalize substate 7 (SAVE-DRAIN WAIT), the sole failing 7->8
    // gate condition is !ShouldSave() -- the suppressed quit-save left GameMan.save_requested set. This
    // clears that spurious flag so the game's OWN advancer passes 7->8->9 and completes RETAINING the
    // player (NOT a state force). Epoch-scoped; no-op on load1 and on a still-progressing load.
    unsafe { maybe_force_finish_stuck_testnet_step() };
    // PASSIVE CONTROLLER-INPUT TRACE (er-effects-input-trace.txt): record real pad edges +
    // semaphore snapshots to er-effects-input-trace.jsonl for USER-DRIVEN runs. Recording only --
    // never blocks, never fabricates; a marker/env-gated no-op by default.
    input_trace_tick();
    // RAWINPUT RECEPTION COUNTER (contamination oracle, user 2026-07-20): install once, unconditionally,
    // so EVERY run records whether the game received user mouse/kb input (input-trace is off by default).
    // Recording only -- never blocks input. bd oracle-must-record-game-input-reception-hook-getrawinputdata.
    ensure_rawinput_counter_installed();
    // LoadlistInit capture: DEFERRED install (attach-time install crashed ER boot -- MinHook patching
    // STEP_MoveMap_LoadlistInit's entry during early boot). Install ONCE the local player is present:
    // post-boot AND after load1's world-load, so no thread is executing LoadlistInit's prologue when
    // MinHook patches it (no race); load2/load3 reloads still CALL LoadlistInit afterwards so the hook
    // fires and captures worldloadlistlistVirtualPath. Idempotent (install-once swap guard). bd
    // loadlist-hook-defer-install-to-player-present-not-attach-2026-07-20.
    if unsafe { PlayerIns::local_player_mut() }.is_ok()
        && let Ok(base) = game_module_base()
    {
        unsafe { install_loadlist_init_capture_hook(base) };
    }
    // REMOVED (bd input-blocking-only-in-harness-during-driving-never-in-product-never-outside-window-
    // 2026-07-23): this used to call enforce_keyboard_game_input_disable() EVERY in-world frame whenever the
    // harness DLL was present + the player was in-world -- i.e. for the WHOLE post-load dwell -- which
    // disabled the user's keyboard (W-move + Escape-menu) for the entire in-world time. That was the
    // camera-only-control bug. Disabling the USER's input is valid ONLY inside the input-harness crate AND
    // ONLY during its active driving/injection window; it must NEVER run in the product during normal
    // in-world play. The can-move probe already scopes its own contamination handling to its brief injection
    // interval (MOVE_PROBE_ACTIVE) and detects (not blocks) any user contamination, so no product-wide
    // keyboard disable belongs here. The user's keyboard is now fully live throughout the dwell.
    // NATIVE-WINDOWS LOADING OVERLAY ownership cycle (bd er-effects-rs-8jz): our separate-window overlay
    // OWNS the screen (SHOW) whenever the local player is absent -- boot, title, and EVERY loading screen
    // (fast-travel, area transitions, death re-load) -- and RELEASES it (HIDE) once the world is loaded and
    // the player exists. This re-owns automatically on each subsequent load. Cheap per-frame check; the
    // overlay thread reads the flag and toggles ShowWindow. No-op off native Windows.
    if is_native_windows() {
        // OWN THE WHOLE LOADING SURFACE (user 2026-07-15): the overlay must keep covering the screen through
        // EVERY loading sequence -- boot, title, and the game's OWN native loading screen -- and release only
        // in settled gameplay. Gating on !player_present alone released too early: PlayerIns becomes valid
        // MID-LOAD (before the world finishes streaming), so the overlay hid and the game's native loading
        // screen (with its own bar) showed through -- the exact regression the user reported. Reuse the same
        // gameplay-idle predicate the portrait pipeline uses (portrait_pipeline_idle_in_gameplay: in-world
        // AND load_done AND no cover up, or the native ProfileSelect menu is open), which stays "not idle"
        // through boot/title/EVERY loading screen and only goes idle in real gameplay. Always own the screen
        // while our own startup save picker is up (it needs the overlay regardless of load state).
        // OWN UNTIL THE NATIVE SCREEN IS ACTUALLY GONE (user 2026-07-15 "if I see the game's native loading
        // screen, we aren't owning it long enough"). portrait_pipeline_idle_in_gameplay (world-reached +
        // load-done + no cover) can flip true while the native NOW-LOADING screen is STILL VISUALLY UP on a
        // fast load, so the overlay released and the native screen flashed through. The native loading screen
        // is rendering iff CS::LoadingScreen::Update is still ticking (LOADING_SCREEN_UPDATE_HITS increments
        // each of its frames; it stops the moment the screen is destroyed). Keep owning while it ticks, plus a
        // short grace to cover its fade-out, so the native screen is never exposed; then release to gameplay.
        let native_loadscreen_up = {
            pub(crate) use er_telemetry::counters::LAST_LOADSCREEN_HITS;
            pub(crate) use er_telemetry::counters::LOADSCREEN_GRACE;
            const LOADSCREEN_GRACE_FRAMES: usize = 12;
            let hits = LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst);
            if LAST_LOADSCREEN_HITS.swap(hits, Ordering::SeqCst) != hits {
                LOADSCREEN_GRACE.store(LOADSCREEN_GRACE_FRAMES, Ordering::SeqCst);
            }
            let g = LOADSCREEN_GRACE.load(Ordering::SeqCst);
            if g > 0 {
                LOADSCREEN_GRACE.store(g - 1, Ordering::SeqCst);
                true
            } else {
                false
            }
        };
        // While the in-world System->Quit ProfileSelect menu is up, do NOT let the pipeline-based term show
        // the overlay -- the re-engaging portrait pipeline would draw our stats/portrait over the live menu
        // (the "ghosting" user-reported 2026-07-15). The actual profile-switch world-load is still covered by
        // `native_loadscreen_up` once its loading screen ticks, so nothing is exposed.
        let profile_menu_up = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0
            || SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE.load(Ordering::SeqCst) != 0;
        // OWN THE SCREEN THE INSTANT A SWITCH IS ARMED (user 2026-07-16): from the slot-click (phase ->
        // CONFIRMED) until the load completes (phase -> IDLE at repro_guards.rs:1286), cover the screen with
        // our loading overlay. Without this, the ~5s world-teardown BEFORE the native loading screen starts
        // ticking left a frozen blank window (Windows said "not responding") so the user couldn't tell the
        // load was working. Phase is IDLE while ProfileSelect is still interactive (the arm sets CONFIRMED
        // only ON the pick), so this never covers the live menu.
        let switch_active =
            SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE;
        let owns_surface = save_picker_overlay_active()
            || native_loadscreen_up
            || switch_active
            || (!profile_menu_up
                && match game_module_base() {
                    Ok(base) => !unsafe { portrait_pipeline_idle_in_gameplay(base) },
                    Err(_) => true,
                });
        NATIVE_OVERLAY_SHOW.store(usize::from(owns_surface), Ordering::SeqCst);
        // NATIVE-WINDOWS SAVE PICKER input (bd er-effects-rs-8wt): the picker LIST already renders
        // via the overlay's shared boot_view_render_frame (overlay_save_picker_onto), but the Wine
        // build drives the picker's input from the D3D12 Present hook -- which never installs on native
        // Windows (composite suppressed on the game device). Drive it here on the game task instead:
        //   * ensure_save_picker_keyboard_hook() installs the GLOBAL WH_KEYBOARD_LL hook on its OWN
        //     message-pumped, time-critical thread. That hook is focus-independent, so keyboard reaches
        //     the picker even though the overlay window is WS_EX_NOACTIVATE and the game keeps focus.
        //   * save_picker_overlay_input_tick() arms the picker when a no-save boot is pending, polls the
        //     gamepad (XInput), and disarms once the pick releases the hold. The keyboard poll inside it
        //     self-skips while the LL hook owns keyboard, so there is no double-apply.
        // Both self-gate on missing_save_selection_pending(), so this is a no-op on a normal (save
        // found) boot. Gated to native Windows so the Wine Present-hook path is never double-polled
        // (the gamepad edge-detection state is shared). catch_unwind matches the Present-hook call site.
        let _ = std::panic::catch_unwind(ensure_save_picker_keyboard_hook);
        let _ = std::panic::catch_unwind(save_picker_overlay_input_tick);
        // Loading-screen character STATS (bd er-effects-rs-rbc): build the game-menu-font stats lines on
        // the GAME THREAD (safe guarded reads of ProfileSummary/PlayerGameData) into STATS_TEXT_CACHE, so
        // the isolated overlay's render thread can re-raster them at screen scale and composite them at the
        // expected loading-screen location (5%/60%, game MenuFont). Content-keyed + self-gates on a captured
        // font + a readable character, so it is a cheap no-op until a character context exists, and updates
        // as early as the data is available -- before the game's own loading screen. On Wine this is built
        // from save_swap_profile_table for the in-swapchain composite; on native Windows that composite is
        // suppressed, so drive the same build here.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            maybe_build_stats_text()
        }));
    }
    // Hardware write-watchpoint on GameMan+0xc30: (re)arm each frame until
    // the save-mount write is caught, so the VEH logs the exact writer. Runs
    // the input block (DInput keyboard + XInput gamepad; the mouse is never blocked
    // and the cursor is never confined), driven from the game task so it is active
    // even when no render callback is running (it does not under the offline launcher
    // at the title). Runs every frame the task ticks -- before the player check -- so a
    // focused window cannot inject foreign keyboard/gamepad input during the own-stepper/
    // autoload probe. Pure suppression, never synthesis.
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
    // Save Game row close-all: finishes the root menu close on a later game-task tick,
    // after the active System submenu has consumed its native close result.
    unsafe { system_quit_save_game_deferred_close_tick() };
    // Save-flow state machine (WP1): after the deferred close, so the frame the close
    // drains is the frame stage 6 -> 7 advances; fires the forced save request once the
    // RAM gates are green and watches the bypassed commit to completion.
    unsafe { save_flow_tick() };
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
        // GPU-FRAME TIMESTAMP ORACLE (goal §3.3 gpu_frame_us; bd er-effects-rs-03ma): once the present
        // hook is up, piggyback timestamp command-lists onto the game queue's ExecuteCommandLists to
        // measure per-frame GPU-busy time (splits the reload-20fps residual into GPU-render vs
        // present-wait). One-shot, self-gated (Wine + telemetry-measurement only), fail-closed.
        unsafe { try_install_gpu_frame_oracle(base) };
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
    if c30_watch_enabled()
        && let Ok(base) = game_module_base()
    {
        let frame =
            C30_WATCH_FRAME_COUNTER.fetch_add(C30_WATCH_HIT_INCREMENT, Ordering::SeqCst) as u64;
        unsafe { maybe_arm_c30_watch(base, frame) };
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
    // Installed UNCONDITIONALLY now (was diagnostic-gated): pure-read pass-through, and it is the only
    // way to ground WHY WorldResWait stalls on the product save_redirect path -- it tracks each
    // WorldBlockRes' phase ([+0x35]) 2->0xa (resident) + FD4 gate ([+0x2f]). Runtime-grounded 2026-07-18:
    // the boot load stalls at WorldResWait (mms 3) with a VALID BlockId + CSRemo idle, so the block-res
    // FD4 file-load is the suspect; this observer surfaces oracle_own_load_wbr_max_phase in product runs.
    let _ = (
        own_load_enabled(),
        own_load_continue_enabled(),
        own_load_pump_enabled(),
        golden_observe_enabled(),
    );
    install_wbr_update_hook();
    // PHASE-3 teardown oracle (bd PHASE3-render-release-is-CommonFinalize): install the OBSERVE-ONLY
    // `_Common_Finalize` counter hook once, unconditionally. Pure pass-through (like the WBR observer), so
    // it never changes teardown behavior; it surfaces oracle_common_finalize_count so a run can measure
    // whether the OUTGOING world's render-release actually fires (flat=in-place bug, +1/switch=fixed).
    install_common_finalize_hook();
    // PRODUCT DEFAULT (no env gate): install the RequestMoveMap BlockId fix detour once. It is a pure
    // passthrough unless ARMED by our own load trigger, so it never affects normal gameplay map
    // transitions; when armed it substitutes a valid saved-map BlockId so the game builds the world-res
    // loadlist path and the load completes + renders instead of stalling at WorldResWait (bd
    // er-effects-rs-um9g / render-handoff-freeze-worldreswait-loadlist-root-2026-07-18).
    install_request_move_map_fix_hook();
    // ARMED SWITCH-RELOAD DIP FIX (bd reload-overlap-fix-design-worldreswait-defer-release-on-streaming-
    // settle-2026-07-24): install the STEP_WorldResWait gate (FUN_140624bd0) defer-release detour once. It
    // is a pure passthrough unless a genuine in-world System->Quit switch reload is ARMED + the default-OFF
    // opt-in marker (er-effects-enable-worldreswait-hold.txt) is present, so it never affects boot, load1,
    // or normal map transitions; when armed it holds movability/loading-close until CSWorldGeomMan geometry
    // streaming settles (bounded fail-soft), removing the movable-while-streaming overlap dip.
    install_worldreswait_gate_hook();
    if ((own_load_enabled() && OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst))
        || golden_observe_enabled())
        && let Ok(base) = game_module_base()
    {
        let gm = game_man_ptr_or_null();
        let player_present = unsafe { PlayerIns::local_player_mut() }.is_ok();
        unsafe { own_load_stream_observe_recurring(base, gm, player_present) };
    }
    // PATH B PRIVATE PUMP (own_load_pump): if own_load_pump_fire built+armed the LoadGame job,
    // tick its Run privately EVERY frame here (the game thread) -- replicating native
    // ExecuteMenuJob's call shape (zero-init MenuJobResult + FD4Time carrying the frame delta)
    // -- to drive self-build -> deser -> m28 stream, then SetState5 on Success. Self-gates on
    // OWN_LOAD_PUMP_JOB != 0 / OWN_LOAD_PUMP_DONE, so it costs nothing until armed+built and
    // never re-pumps once terminal. Must run THROUGH the loading screen (player absent), so it
    // is here in the recurring game task, before the player check. Pure native call + reads.
    // FPS oracle (goal 2026-07-19: stable, load1-baseline-comparable framerate). EMA of the frame delta +
    // per-epoch worst frame time. Unconditional, cheap; read by the telemetry as oracle_fps / oracle_min_fps.
    {
        let d = task_data.delta_time.time;
        if d > 0.0 && d < 1.0 {
            let us = (d * 1_000_000.0) as u32;
            let prev = crate::constants::FRAME_TIME_EMA_US.load(Ordering::Relaxed);
            crate::constants::FRAME_TIME_EMA_US
                .store(((prev / 10) * 9 + us / 10).max(1), Ordering::Relaxed);
            let ep = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
            if crate::constants::FRAME_TIME_WORST_EPOCH.swap(ep, Ordering::Relaxed) != ep {
                crate::constants::FRAME_TIME_WORST_US.store(0, Ordering::Relaxed);
            }
            crate::constants::FRAME_TIME_WORST_US.fetch_max(us, Ordering::Relaxed);
        }
    }
    if own_load_pump_enabled()
        && let Ok(base) = game_module_base()
    {
        let gm = game_man_ptr_or_null();
        let frame_delta = task_data.delta_time.time;
        unsafe { own_load_pump_tick(base, gm, frame_delta) };
    }
    // DIRECT "Continue pressed" trigger: at the settled main menu (post press-any-button,
    // GameMan set up), write the exact bit the native selector consumes
    // (*(TitleFlowContext+0x14c)=1), invoke the selector to BUILD the LoadGame job, and
    // PushBackJob it to the dialog queue. Self-gates + fires once; no input. Then DRAIN the
    // queue each frame (FUN_1407a90f0) so the posted job runs to completion (deser+world).
    if fire_tfc_continue_enabled()
        && let Ok(base) = game_module_base()
    {
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
    if pab_advance_enabled()
        && let Ok(base) = game_module_base()
    {
        unsafe { install_pab_advance_hook(base) };
        if !native_profile_capture_enabled() {
            unsafe { maybe_set_title_accept_byte(base) };
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
    if title_anim_speedup_enabled()
        && let Ok(base) = game_module_base()
    {
        unsafe { install_title_anim_speed_hook(base) };
        // READ-ONLY native state-transition timeline (menu-build-overlap lever
        // "look before acting" instrument): logs every SetState(owner,int) with a
        // timestamp so we learn exactly when BeginTitle(3) fires and whether the
        // 05_000_Title build has headroom to start earlier. Save-safe pass-through.
        unsafe { install_title_setstate_trace_hook(base) };
        // Failed same-session reload guard experiments are explicit opt-in only; canonical
        // semaphore-diff runs must remain observational.
        if movemapstep_step_move_map_gate_hold_enabled() {
            unsafe { install_movemapstep_step_move_map_gate_hook(base) };
        }
        // STEP_MoveMap_Update finalize-defer detour: the root fix for the warm-reload premature
        // teardown (bd er-effects-rs-9fmm). Self-gated internally on the er-effects-reload-defer.txt
        // marker + a committed reload epoch, so installing it is inert until a marked reload runs.
        unsafe { install_ingamestep_step_movemap_update_defer_hook(base) };
        // Child-done-query override: prevent the PREMATURE MoveMapStep child teardown that strands
        // load2 (FUN_140eb5550 returns done at field25=0 -> STEP_MoveMap_Update tears the child down
        // -> advancer stops). Isolated to the MoveMapStep child (rcx==mms+0x108) on a committed
        // reload; load1 untouched. bd COMPLETE-CHAIN-load2-child-torndown-early-fun140eb5550-done.
        unsafe { install_child_done_query_override_hook(base) };
        // NOTE: the LoadlistInit capture hook is NOT installed here -- installing it at DLL attach
        // crashed ER boot (MinHook patching STEP_MoveMap_LoadlistInit's entry during early boot). It
        // is deferred to the first player-present frame instead (see the tick below). bd
        // loadlist-hook-defer-install-to-player-present-not-attach-2026-07-20.
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
    // Missing-save picker: hold the native title menu-open until the pick, so its Continue/Load rows
    // build against the picked save (enabled) instead of an empty ProfileSummary. Partners the
    // ShowProgressJob save-check hold above; installed unconditionally because the hook self-gates on
    // `missing_save_selection_pending()` (pass-through on an early pick / no picker). Must arm before
    // the native auto-menu-open (~+38s). Fixes the late-pick softlock (bd er-effects-rs-ns4n follow-up).
    install_title_open_menu_suppress_hook();
    // DIAGNOSTIC (gated by er-effects-grsysmsg-log.txt): log the GR_System_Message ids the
    // title flow fetches after menu-open, to DEFINITIVELY name the menu-open MessageBoxDialogs
    // (connection 4101/4102/4190 vs save 70000/4191) instead of guessing. Self-gates once.
    // Also install whenever a save load is expected (not telemetry-only / not trace):
    // the same GetGR_System_Message hook carries the corrupted-save SEMAPHORE
    // (oracle_corrupted_save_seen_id), so a load probe records the "save data is corrupted"
    // popup as RAM-read telemetry instead of a one-off on-screen image.
    if grsysmsg_log_enabled() || (!save_override_telemetry_only() && !save_trace_enabled()) {
        install_gr_sysmsg_log_hook();
    }
    // Anti-anti-debug (ported from ProDebug, correct base): neutralize FromSoft's
    // timed anti-debug so debug exceptions / our INT3 breakpoints reach our VEH.
    // Runs ONCE, BEFORE arming breakpoints, from the game task (game up, .text
    // decrypted) -- our own controlled timing, not the LazyLoader's.
    if anti_antidebug_enabled()
        && let Ok(base) = game_module_base()
    {
        unsafe { apply_anti_antidebug_once(base) };
    }
    // Software (INT3) breakpoints from er-effects-breakpoints.txt: install once.
    // The VEH (crash logger) logs every hit's register/stack context + re-arms.
    if sw_breakpoints_enabled()
        && let Ok(base) = game_module_base()
    {
        unsafe { install_sw_breakpoints_once(base) };
    }
    // STAY-ACTIVE: force ER's input-accept flag so a virtual gamepad keeps driving the
    // menus while ER is UNFOCUSED (user can work elsewhere during a golden capture). ER
    // clears [DLUID+0x88d] each frame when it isn't GetActiveWindow; re-set it to 1.
    if stay_active_enabled()
        && let Ok(base) = game_module_base()
    {
        // DLUID (input-device-manager) singleton VA 0x14485dc18.
        const DLUID_SINGLETON_RVA: usize = RuntimeGlobalRva::DluidInputManager as usize;
        #[repr(C)]
        struct DluidInputManagerLayout {
            unknown_000: [u8; 0x88d],
            input_active: u8,
        }
        const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize =
            core::mem::offset_of!(DluidInputManagerLayout, input_active);
        const INPUT_ACTIVE: u8 = true as u8;
        const NULL_DLUID: usize = NULL_MODULE_BASE;
        let dluid = unsafe { safe_read_usize(base + DLUID_SINGLETON_RVA) }.unwrap_or(NULL_DLUID);
        // Defensive: only write once the flag byte is confirmed READABLE (so a
        // not-yet-initialized or bad singleton ptr can never fault the game thread).
        if dluid != NULL_DLUID
            && unsafe { safe_read_usize(dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) }.is_some()
        {
            unsafe { *((dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) as *mut u8) = INPUT_ACTIVE };
        }
    }
}
