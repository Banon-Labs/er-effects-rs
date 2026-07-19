//! Runtime lifecycle seams for attach-time experiment hook installation.
//!
//! Keep hook ordering here behavior-preserving: these functions are thin orchestration
//! wrappers around code that previously lived inline in `DllMain`.

use super::*;

// === SWITCH-HARNESS DISCOVERY (agent-owned; user authorized self-driving 2026-07-15) ===
// Highest-value feasibility probe for the autonomous consecutive-switch harness: does injecting the
// menu-open key via the DInput keyboard BLOCK actually open the in-game menu on NATIVE WINDOWS? Under
// Proton the game reads DInput keyboard (where this injection works); native Windows may use raw input,
// in which case injection never reaches the menu and the harness needs a different vehicle (PostMessage).
// Enabled ONLY by ER_EFFECTS_SWITCH_HARNESS_DISCOVERY=1 or a marker file next to the game exe; OFF for
// product. Once in-world+stable it blocks the keyboard, pulses DIK_ESCAPE once, and (via run_post) logs
// every MenuWindowJob::Run filename that appears -- so the log reveals whether a menu opened and its
// structure. Then it unblocks. No effect on the default/product path.
const HARNESS_DISC_DIK_ESCAPE: u8 = 0x01;
static HARNESS_DISC_STABLE: AtomicUsize = AtomicUsize::new(0);
static HARNESS_DISC_PHASE: AtomicUsize = AtomicUsize::new(0); // 0 wait,1 press,2 release,3 observe,4 done
static HARNESS_DISC_PHASE_FRAME: AtomicUsize = AtomicUsize::new(0);
static HARNESS_DISC_SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

// ENV-GATE RATIONALE: ER_EFFECTS_SWITCH_HARNESS_DISCOVERY=1 arms the agent-owned switch-harness
// feasibility probe (once in-world+stable it blocks the keyboard, pulses DIK_ESCAPE once to try to open
// the in-game menu, logs every MenuWindowJob::Run filename, then unblocks). OFF for product; no save
// write, no mount change on the default path.
pub(crate) fn switch_harness_discovery_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_SWITCH_HARNESS_DISCOVERY").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("er-effects-switch-harness-discovery.txt")
        .exists()
}

/// Called from run_post for every MenuWindowJob::Run filename during discovery: log each distinct
/// name once so the menu structure is revealed without per-frame spam.
pub(crate) fn switch_harness_note_menu_filename(name: &str) {
    if name.is_empty() {
        return;
    }
    if let Ok(mut seen) = HARNESS_DISC_SEEN.lock() {
        if !seen.iter().any(|n| n == name) {
            seen.push(name.to_string());
            append_autoload_debug(format_args!(
                "switch-harness-disc: MenuWindowJob::Run filename seen = '{name}' (distinct #{})",
                seen.len()
            ));
        }
    }
}

pub(crate) unsafe fn switch_harness_discovery_tick() {
    if !switch_harness_discovery_enabled() {
        return;
    }
    let phase = HARNESS_DISC_PHASE.load(Ordering::SeqCst);
    if phase == 4 {
        return;
    }
    let player_present = unsafe { PlayerIns::local_player_mut() }.is_ok();
    if !player_present {
        HARNESS_DISC_STABLE.store(0, Ordering::SeqCst);
        return;
    }
    let ib = InputBlocker::get_instance();
    if phase == 0 {
        let stable = HARNESS_DISC_STABLE.fetch_add(1, Ordering::SeqCst) + 1;
        if stable < 180 {
            return; // ~3s settled in-world before touching input
        }
        let _ = unsafe { ib.install_hooks() };
        ib.block(InputFlags::Keyboard);
        HARNESS_DISC_PHASE.store(1, Ordering::SeqCst);
        HARNESS_DISC_PHASE_FRAME.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "switch-harness-disc: in-world+stable -> keyboard BLOCKED, pulsing DIK_ESCAPE (0x01) to test whether DInput injection opens the native-Windows menu"
        ));
        return;
    }
    let pf = HARNESS_DISC_PHASE_FRAME.fetch_add(1, Ordering::SeqCst);
    if phase == 1 {
        ib.set_injected_key(HARNESS_DISC_DIK_ESCAPE);
        if pf >= 4 {
            HARNESS_DISC_PHASE.store(2, Ordering::SeqCst);
            HARNESS_DISC_PHASE_FRAME.store(0, Ordering::SeqCst);
        }
    } else if phase == 2 {
        ib.set_injected_key(0);
        if pf >= 10 {
            HARNESS_DISC_PHASE.store(3, Ordering::SeqCst);
            HARNESS_DISC_PHASE_FRAME.store(0, Ordering::SeqCst);
        }
    } else if phase == 3 {
        if pf >= 150 {
            ib.set_injected_key(0);
            ib.unblock(InputFlags::Keyboard);
            HARNESS_DISC_PHASE.store(4, Ordering::SeqCst);
            let count = HARNESS_DISC_SEEN.lock().map(|s| s.len()).unwrap_or(0);
            append_autoload_debug(format_args!(
                "switch-harness-disc: observation done -> keyboard UNBLOCKED. distinct MenuWindowJob filenames seen after ESC = {count} (if a game menu like 02_000_IngameTop appeared, DInput injection WORKS on native Windows)"
            ));
        }
    }
}

pub(crate) fn tick_before_player_lookup(task_data: &FD4TaskData) {
    unsafe { switch_harness_discovery_tick() };
    // PASSIVE CONTROLLER-INPUT TRACE (er-effects-input-trace.txt): record real pad edges +
    // semaphore snapshots to er-effects-input-trace.jsonl for USER-DRIVEN runs. Recording only --
    // never blocks, never fabricates; a marker/env-gated no-op by default.
    input_trace_tick();
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
            static LAST_LOADSCREEN_HITS: AtomicUsize = AtomicUsize::new(0);
            static LOADSCREEN_GRACE: AtomicUsize = AtomicUsize::new(0);
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
    // Session-local sort defaults: CSMenuSystemSaveLoad initializes the target
    // categories to Item Type every process; write once so vanilla remembers the
    // configured Order-of-Acquisition defaults across later character loads.
    apply_default_menu_sort_preferences_once();
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
            let frame =
                C30_WATCH_FRAME_COUNTER.fetch_add(C30_WATCH_HIT_INCREMENT, Ordering::SeqCst) as u64;
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
    // PRODUCT DEFAULT (no env gate): install the RequestMoveMap BlockId fix detour once. It is a pure
    // passthrough unless ARMED by our own load trigger, so it never affects normal gameplay map
    // transitions; when armed it substitutes a valid saved-map BlockId so the game builds the world-res
    // loadlist path and the load completes + renders instead of stalling at WorldResWait (bd
    // er-effects-rs-um9g / render-handoff-freeze-worldreswait-loadlist-root-2026-07-18).
    install_request_move_map_fix_hook();
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
            let dluid =
                unsafe { safe_read_usize(base + DLUID_SINGLETON_RVA) }.unwrap_or(NULL_DLUID);
            // Defensive: only write once the flag byte is confirmed READABLE (so a
            // not-yet-initialized or bad singleton ptr can never fault the game thread).
            if dluid != NULL_DLUID
                && unsafe { safe_read_usize(dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) }.is_some()
            {
                unsafe { *((dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) as *mut u8) = INPUT_ACTIVE };
            }
        }
    }
}

pub(crate) fn install_title_visual_startup_hooks() {
    // Passive title-resource observer is deliberately independent of the cover/hide bundle: recent
    // branches have kept the stock logo invisible, so resource-path proof must not depend on any
    // visual/logo-hide state.
    if title_menu_resource_observer_enabled() {
        START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-resource-observer".to_owned())
                .spawn(install_title_menu_resource_acquire_observer_hook);
        });
    }

    // Stats-panel native text: arm the 05_010 GFX runtime edit (face box removed + `ErStats` field
    // added; served in-place by the Scaleform file-open observer) and install the row-populate hook
    // + the named-child binder hook (idempotent) so the character's attribute line renders in the
    // game's own MenuFont_01 in its own row field. Independent of the title-cover conditions below
    // -- it must run on every stats-panel product path, so it is gated on `stats_panel_enabled()`
    // directly (product lever; no per-feature env gate).
    if stats_panel_enabled() {
        START_PROFILE_STATS_TEXT.call_once(|| {
            PROFILE_05_010_RUNTIME_EDIT_ARMED.store(1, Ordering::SeqCst);
            let _ = std::thread::Builder::new()
                .name("er-effects-profile-stats-text".to_owned())
                .spawn(|| {
                    // The row-populate hook drives the per-slot attribute push; the named-child binder
                    // hook still runs the title-cover duties. Both are idempotent.
                    install_profile_row_populate_hook();
                    install_title_scene_obj_proxy_named_child_bind_hook();
                });
        });
    }
    // Title-cover masquerade Part A: install the BeginTitle `05_000_Title` hook as early as
    // splash/foreground patches, before STEP_BeginTitle can build the native title Scaleform. This
    // does NOT touch STEP_Wait or CSMenuMan+0x21; it preserves the native MenuWindowJob and hides
    // only its draw bit from the MenuWindowJob::Run/FadeIn path.
    if title_native_menu_visual_suppression_enabled() {
        START_TITLE_NATIVE_MENU_VISUAL_SUPPRESS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-cover-part-a".to_owned())
                .spawn(install_title_native_menu_visual_suppression_hook);
        });
        START_TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-cover-render".to_owned())
                .spawn(install_title_native_menu_visual_render_suppression_hook);
        });
        START_TITLE_LOGO_FORCE_HIDDEN.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-logo-force-hidden".to_owned())
                .spawn(install_title_logo_force_hidden_hooks);
        });
        START_TITLE_LOGO_START_LOGIN_HIDE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-logo-start-login-hide".to_owned())
                .spawn(install_title_logo_start_login_hide_hook);
        });
        START_TITLE_PAB_INFORMATION_COVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-pab-cover".to_owned())
                .spawn(install_title_pab_information_visual_hook);
        });
        START_TITLE_GFX_VALUE_SET_VISIBLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-gfx-visible".to_owned())
                .spawn(install_title_gfx_value_set_visible_hook);
        });
        START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-child-bind".to_owned())
                .spawn(install_title_scene_obj_proxy_named_child_bind_hook);
        });
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
        START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-resource-observer".to_owned())
                .spawn(install_title_menu_resource_acquire_observer_hook);
        });
        // Do not install the independent custom-cover MenuWindowJob pump here. Runtime artifact
        // product-continue-direct-20260628-121039 proved that pumping a separate 01_900_Black job
        // keeps job+0x130 live and stalls the title flow before player/world. Future cover work must
        // use an epilogue-neutral path (mutate an already-scheduled title surface/resource, or prove
        // explicit completion semantics before adding an independent MenuWindowJob).
        START_TITLE_FLOW_CONTEXT_RECORD_REGULATION.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-tfc-record-fix".to_owned())
                .spawn(install_title_flow_context_record_regulation_fix_hook);
        });
    } else if title_resource_memory_gfx_enabled() {
        // Branch-owned `05_001_Title_Logo` replacement: keep TitleBack visible, but hide the later
        // title text layers (`PRESS ANY BUTTON` / Continue-ish title information) so the custom
        // resource is not overdrawn by native text. Do not install the TitleBack/logo hide hooks here.
        START_TITLE_PAB_INFORMATION_COVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-latch".to_owned())
                .spawn(install_title_pab_information_visual_hook);
        });
        START_TITLE_GFX_VALUE_SET_VISIBLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-gfx-visible".to_owned())
                .spawn(install_title_gfx_value_set_visible_hook);
        });
        START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-child-bind".to_owned())
                .spawn(install_title_scene_obj_proxy_named_child_bind_hook);
        });
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
    } else if native_profile_capture_enabled() {
        // Native ProfileSelect diagnostic: install only the passive Scaleform bind observer. Do not
        // install title-cover/custom-cover hooks; this mode is specifically meant to prove native
        // ProfileSelect/profile-renderer provenance without the product cover mutation path.
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-native-profile-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
    }

    // Now-loading background forge: install the replace-bind hook early (well before the ~17s
    // now-loading-screen lifecycle) so it is resident when the first MENU_Load_ background is produced.
    // It is fail-open (non-matching symbols/build failures tail-call original). Default behavior now keeps
    // the selected boot background continuous through the native loading GFX background; users can opt out
    // with `persist_boot_background_to_loading_screen = false` in DLL-adjacent er-effects.toml. On the
    // portrait-lookat path, only install when a real background source exists, so a no-image run does not
    // accidentally forge the diagnostic checker behind the live portrait overlay.
    let persist_loading_bg = crate::config::persist_boot_background_to_loading_screen_enabled();
    if !portrait_lookat_enabled() || (persist_loading_bg && boot_bg_image_rgba_clone().is_some()) {
        START_LOADING_BG_REPLACE_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-loading-bg-portrait".to_owned())
                .spawn(install_loading_bg_replace_bind_hook);
        });
    }
    // er-effects-rs-jsm PIVOT: suppress the native loading tips (our overlay renders player-stats text
    // instead). Install at ATTACH -- BEFORE the KnowledgeLoadingScreen ctor's one-shot initial tip (~15s),
    // else the first tip is already set and only later cycles are suppressed. Lookat (feature) path only.
    if portrait_lookat_enabled() {
        START_TIP_SUPPRESSION.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-tip-suppress".to_owned())
                .spawn(install_tip_suppression_hook);
        });
    }
    // er-effects-rs-y22i: ALWAYS-ON Scaleform descriptor-heap null guard (native-Windows crash
    // 0xec95d1). NOT feature-gated -- it is a crash guard, a transparent passthrough when the null
    // never occurs. Installed at attach so it is live before the first loading-screen composite.
    START_SCALEFORM_GUARD.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-scaleform-guard".to_owned())
            .spawn(install_scaleform_descriptor_guard);
    });
    // D3D12 PRESENT OVERLAY: the deterministic display path -- draw the captured portrait directly onto the
    // swapchain backbuffer when the now-loading screen is up (the in-pipeline forge/Scaleform routes cannot
    // drive the displayed image). Install only on the portrait path (diagnostic), via the dummy-swapchain
    // vtable technique. Phase 1 is log-only (proves the hook fires) before any backbuffer write.
    if portrait_lookat_enabled() {
        START_PRESENT_OVERLAY.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-present-overlay".to_owned())
                .spawn(install_present_overlay_hook);
        });
    }
    // NATIVE-WINDOWS LOADING OVERLAY (bd er-effects-rs-8jz): a SEPARATE topmost window with our OWN D3D12
    // device/swapchain that OWNS the screen during boot + every loading screen. On native Windows we
    // cannot composite on the game's shared device (it crashes the strict driver), so this is the only
    // safe display path there. Wine/vkd3d keeps the in-swapchain composite above. Install is idempotent.
    if is_native_windows() {
        install_native_overlay();
    }
}

pub(crate) fn install_profile_and_system_quit_hooks() {
    // Portrait-renderer teardown SPARE hook: keep the loaded character's portrait renderer alive past the
    // Continue teardown so we can drive realtime look-at + render it post-Continue (the persistent-model
    // path -- the cycling menu can't show a stable portrait). The hook self-gates on product_autoload and
    // only spares a renderer whose model is BUILT (the blank-renderer misfire is guarded in the hook).
    START_PROFILE_RENDERER_TEARDOWN_SPARE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-portrait-spare".to_owned())
            .spawn(install_profile_renderer_teardown_spare_hook);
    });

    // Profile-renderer table guard (er-effects-rs-j3r): before the native per-slot thumbnail
    // builder runs, log a degraded 10-slot table, REBUILD a fully-empty one via the engine's own
    // table setup (only the TitleTopDialog ctor ever calls it natively, so nothing repopulates it
    // across our in-world ProfileSelect reopens -- the 3rd open crashed on the empty table), and
    // fail-soft skip the builder if a slot would still null-deref at [entry+0x754].
    START_PROFILE_SELECT_TABLE_DIAG.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-profileselect-table-diag".to_owned())
            .spawn(install_profile_select_table_diag_hook);
    });

    // System -> Quit Game buttons: always-on multi-slot layout patch plus cloned rows for native
    // 05_010_ProfileSelect and opening the env-provided save folder. Slot activation from that
    // injected in-world route is separately guarded by the System-Quit load flow.
    START_SYSTEM_QUIT_DUPLICATE_BUTTON_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-system-quit-load".to_owned())
            .spawn(install_system_quit_duplicate_button_hook);
    });

    // Title Continue confirm guard (0x140b0e180): while a System->Quit->Load-Profile switch is
    // active, drive ONE fresh feed-deserialize of the PICKED slot before the confirm streams, so
    // the clean-title reload loads the picked character instead of re-streaming the stale
    // pre-switch state (bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02).
    // Installed unconditionally (single MinHook per address -- this detour also carries the
    // continue-trace CAP logging); pure passthrough outside an active switch.
    START_SYSTEM_QUIT_CONTINUE_CONFIRM_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-system-quit-continue-confirm".to_owned())
            .spawn(install_system_quit_continue_confirm_hook);
    });

    // READ-ONLY teardown-requester trace: EzChildStepBase::RequestFinish. Identifies WHO requests
    // the in-world MoveMapStep child's finish -- the post-switch reload bounce is a stale finish
    // request hitting the freshly-created map session (er-effects-rs-qwj investigation).
    START_SYSTEM_QUIT_CHILD_FINISH_TRACE_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-system-quit-child-finish-trace".to_owned())
            .spawn(install_system_quit_child_finish_trace_hook);
    });
}

pub(crate) fn install_boot_diagnostics_and_trace_hooks() {
    // MenuWindow latch: install the SceneObjProxy ctor hook (0x14074a700) as early as the
    // splash-skip / online-disable patches, from a thread, so it lands BEFORE the title state
    // machine builds the title dialog during boot. On each VALID call it latches rdx (the engine-
    // verified host MenuWindow*) for the live-dialog Load-Game path; pure latch + passthrough.
    // OPT-IN (off by default): only install when `menu_window_latch_enabled()` is set
    // (env ER_EFFECTS_MENU_WINDOW_LATCH=1 OR GAME_DIR file er-effects-menu-window-latch.txt).
    // When off, the hook is never installed (no MinHook, no detour) -- a clean run has neither.
    if menu_window_latch_enabled() || product_autoload_enabled() || native_profile_capture_enabled()
    {
        START_MENU_WINDOW_LATCH.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-menu-window-latch".to_owned())
                .spawn(install_menu_window_latch_hook);
        });
    }

    // Native/asset-backed policy-window oracle: hook the TosTitle constructor early in product
    // autoload runs. Any hit means the Privacy/ToS surface was constructed and the runtime proof is
    // invalid; this is detection only, never auto-accept.
    if product_autoload_enabled() {
        START_POLICY_TOS_TITLE_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-policy-oracle".to_owned())
                .spawn(install_policy_tos_title_hook);
        });
        START_SERVER_STATUS_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-server-status-oracle".to_owned())
                .spawn(install_server_status_hook);
        });
    }

    // SAVE-SAFE c30-writer diagnostic: install the MinHook on the SOLE GameMan+0xc30
    // writer 0x67bd70 UNCONDITIONALLY at process attach (same early-attach pattern as the
    // MenuWindow latch). Pure passthrough + log of the c30-write gate, c30 before/after,
    // and a window of the resident save buffer -- NO SetState5, NO save write, harmless.
    // OPT-IN (off by default): only install when `c30_writer_diag_enabled()` is set
    // (env ER_EFFECTS_C30_DIAG=1 OR GAME_DIR file er-effects-c30-diag.txt). When off, the
    // hook is never installed (no MinHook, no detour on the hot 0x67bd70 deserialize path).
    if c30_writer_diag_enabled() {
        START_C30_WRITER_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-c30-writer-hook".to_owned())
                .spawn(install_c30_writer_hook);
        });
    }

    if safe_input_path().exists() {
        START_SAFE_INPUT_HOOKS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-safe-input-hooks".to_owned())
                .spawn(install_safe_input_hooks);
        });
    }
    // Observe-only user32 window-reconfiguration timeline (bd er-effects-rs-rzow): installed at
    // attach so CreateWindowExW is covered before the game builds its startup window. Pure
    // passthrough logging/counting; the RAM semaphore for the mid-boot fullscreen transition
    // whose XWayland servicing blacks the presented surface for a few frames.
    START_WINDOW_RECONFIG_OBSERVER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-winreconfig-observer".to_owned())
            .spawn(install_window_reconfig_observer_hooks);
    });
    if trace_continue_enabled() && !continue_trace_disabled() {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_CONTINUE_TRACE_REQUESTED,
            BOOTSTRAP_DETAIL_START,
        );
        START_CONTINUE_TRACE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-continue-trace".to_owned())
                .spawn(install_continue_trace_hooks);
        });
    }
}
