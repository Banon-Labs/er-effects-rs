pub(crate) use er_telemetry::counters::AUTOLOAD_HANDOFF_PARENT_STATE_FIX_COUNT;

fn poll_cached_mms18_ending_request_advancer() {
    // Native full deserialize owns GameMan::warp_requested and MoveMapStep::CheckReturnToTitle
    // consumes/autoclears it at finalize case 8. Agent-side 0x5d or warp pulses finalize by tearing
    // down the loaded world. The only post-finalize cleanup we own is menuData+0x5e: native sets it
    // while walking 12a->case8, but leaves it true after mms leaves 18; if it remains true into the
    // resident world, the player is torn down about a second later.
    let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if quickload_phase < SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let Some(md) = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
        .filter(|m| *m > 0x10000)
        .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
        .filter(|d| *d > 0x10000)
    else {
        return;
    };
    let md_5d = unsafe { safe_read_u8(md + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) }
        .map(|b| b as i32)
        .unwrap_or(-1);
    let md_5e = unsafe { safe_read_u8(md + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) }
        .map(|b| b as i32)
        .unwrap_or(-1);
    if md_5e != 1 || md_5d != 0 {
        return;
    }
    let owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
    let ingame = if owner != TITLE_OWNER_SCAN_START_ADDRESS && owner > 0x10000 {
        unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
            .filter(|ig| *ig != TITLE_OWNER_SCAN_START_ADDRESS && *ig > 0x10000)
    } else {
        None
    };
    let request_code = ingame
        .and_then(|ig| unsafe { safe_read_i32(ig + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET) })
        .unwrap_or(-1);
    let mms_step = ingame
        .and_then(|ig| unsafe { safe_read_usize(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) })
        .filter(|mms| *mms != TITLE_OWNER_SCAN_START_ADDRESS && *mms > 0x10000)
        .and_then(|mms| unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_RE_OFFSET) })
        .unwrap_or(-1);
    if mms_step == -1 && request_code == INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING {
        unsafe {
            *((md + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) as *mut u8) = 0;
        }
        let n = ENDING_REQUEST_SET_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "ENDING-FLAG POST-FINALIZE CLEAR #{n}: cleared menuData+0x5e after mms left 18 (phase={quickload_phase} requestCode={request_code} mms={mms_step}); native warp already autocleared"
        ));
    }
}

fn poll_autoload_handoff_parent_state_guard() {
    // TITLE_STEP_END_FLOW (7) / TITLE_STEP_END_FLOW_WAIT (8): enum-backed teardown-state constants
    // (constants::stats_panel_background), shared with the product-core parent-fix.
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        != SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
        || SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) == 0
    {
        return;
    }
    let owner = PRODUCT_CORE_LAST_OWNER.load(Ordering::SeqCst);
    if owner == TITLE_OWNER_SCAN_START_ADDRESS || owner <= 0x10000 {
        return;
    }
    let Some(ingame) = (unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) })
        .filter(|ig| *ig != TITLE_OWNER_SCAN_START_ADDRESS && *ig > 0x10000)
    else {
        return;
    };
    let committed =
        unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }.unwrap_or(-1);
    let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }.unwrap_or(-1);
    let parent_is_ending = matches!(committed, TITLE_STEP_END_FLOW | TITLE_STEP_END_FLOW_WAIT)
        || matches!(requested, TITLE_STEP_END_FLOW | TITLE_STEP_END_FLOW_WAIT);
    if !parent_is_ending {
        return;
    }
    let request_code =
        unsafe { safe_read_i32(ingame + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET) }.unwrap_or(-1);
    let mms_step = unsafe { safe_read_usize(ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) }
        .filter(|mms| *mms != TITLE_OWNER_SCAN_START_ADDRESS && *mms > 0x10000)
        .and_then(|mms| unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_RE_OFFSET) })
        .unwrap_or(-1);
    unsafe {
        *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *mut i32) = TITLE_STEP_GAME_STEP_WAIT;
        *((owner + TITLE_OWNER_STATE_OFFSET) as *mut i32) = TITLE_STEP_GAME_STEP_WAIT;
    }
    let n = AUTOLOAD_HANDOFF_PARENT_STATE_FIX_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "AUTOLOAD-HANDOFF PARENT STATE FIX #{n}: TitleStep {committed}/{requested} -> GameStepWait(6) during handoff (InGameStep=0x{ingame:x} requestCode={request_code} mms={mms_step}); prevents EndFlow/EndFlowWait returning the loaded world to title"
    ));
}

/// RAII timer: records the DLL main game-task body duration (any return path) into GAME_TASK_LAST_US,
/// to split a DLL per-frame CODE cost from a game-side loop cost for the playable-window fps.
struct GameTaskTimer(std::time::Instant);
impl Drop for GameTaskTimer {
    fn drop(&mut self) {
        er_telemetry::counters::GAME_TASK_LAST_US
            .store(self.0.elapsed().as_micros() as usize, Ordering::SeqCst);
    }
}

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
                let _gt = GameTaskTimer(std::time::Instant::now());
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
                tick_before_player_lookup(task_data);
                poll_autoload_handoff_parent_state_guard();
                // Startup save-picker: input/navigation runs on the render thread (the Present hook),
                // the only thread that reads OS keys under Wine. Only the one-shot pick COMPLETION
                // (redirect + MinHook install) runs here on the game task -- it is alive at pick time
                // (loading starts only after the pick releases the hold).
                save_picker_overlay_process_completion();
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
                            // The missing-save picker cannot set a config slot; instead its
                            // character sub-picker records the chosen slot here. Configured slots
                            // still win via `state.autoload.slot()`.
                            state
                                .autoload
                                .slot()
                                .or_else(missing_save_picker_selected_slot)
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
                // CAN-MOVE PROBE (2026-07-18, user-directed): in-world, inject a forward stick and prove
                // the character actually MOVES for >=60 consecutive frames. Movement is the ONLY signal
                // that distinguished a playable load from a frozen one (the render/draw_group oracles read
                // FALSE even for a visibly-rendered, controllable load). Frozen loads never accumulate.
                // Game-thread only, so driving input here is safe.
                // Run the move-probe whenever in-world EXCEPT during active MENU-NAV (OPEN_MENU..CONFIRM),
                // where an injected forward stick would move the menu cursor. WAIT_WORLD(0) / WAIT_RELOAD(7)
                // / DONE(6) are in-world settle states -- the probe MUST run there (that is where load1 and
                // each reload prove movement). NB: sq_repro_actively_driving() returns TRUE for WAIT_WORLD
                // (it blocks during boot), which wrongly skipped load1's proof -- so gate on the STATE range
                // directly, not that.
                let sq_menu_nav = system_quit_repro_enabled() && {
                    let st = SQ_REPRO_STATE.load(Ordering::SeqCst);
                    (SQ_REPRO_STATE_OPEN_MENU..=SQ_REPRO_STATE_CONFIRM).contains(&st)
                };
                // Only inject once the char is actually RENDERED in-world (render_group 1c4 + enable_render
                // 1c5), NOT merely present. `player present` goes true mid-load (mms=13, ~14s before
                // render_group), and injecting there latched an invalid DISPROVEN before the char could be
                // controllable -- then the verdict was frozen and never re-tested (run 092119: verdict at
                // t=79.9s during loading; render_group did not fire until t=86s). Gating on the rendered
                // state makes the probe test movability at the stable in-world point, so a DISPROVEN verdict
                // means the char genuinely did not move, not that we injected before the world was up.
                let char_rendered = player.chr_ins.chr_flags1c4.is_render_group_enabled()
                    && player.chr_ins.chr_flags1c5.enable_render();
                if !sq_menu_nav && char_rendered {
                    let p = player.chr_ins.modules.physics.position;
                    crate::experiments::can_move_probe::tick((p.0, p.1, p.2));
                }
                // PROGRAMMATIC SWITCH TRIGGER (2026-07-18): poll the harness switch-slot control file and,
                // when a new (in-world, resident) request appears with no switch in flight, arm a menu-free
                // switch (menuData+0x5d=1 teardown -> own_load_switch_reload_fire). Replaces the brittle
                // simulated-input autopilot for repeatable multi-character loading. Self-gates (phase IDLE +
                // world resident @ step 18 + mtime change), so an every-frame call is cheap and safe.
                poll_cached_mms18_ending_request_advancer();
                if let Ok(base) = game_module_base() {
                    unsafe { poll_switch_slot_control_file(base) };
                }
                // SPURIOUS RETURN-TITLE ARM DISARM (2026-07-18, bd angre-reload-full-causal-chain-and-fix,
                // refined by repeatable-multi-save-consolidated-plan-2026-07-18).
                // Root cause of the angrE repeated-load crash: the boot autoload navigates the ProfileSelect
                // LOAD flow, which trips `system_quit_arm_quickload_autoload` and arms a post-load return-title
                // reload (QUICKLOAD_PHASE = RETURN_TITLE_REQUESTED) of the character we JUST loaded. Load #1 then
                // completes and is stable in-world, but because the phase stays armed the in-world branch below
                // keeps driving product_core_autoload_tick until the return-title chain submits, tears down the
                // good load, and the reload sticks at MoveMapStep 18 and crashes (game assert AV 0x1eb9999).
                // DISCRIMINATOR: the earlier pure time-based gate (disarm after N continuous armed in-world
                // frames) also cancelled GENUINE cross-slot/cross-file switches whose old world lingers past
                // the threshold (the switch-regression). The correct, index-space-free discriminator is the
                // player-presence AT ARM TIME: the spurious boot self-reload arms from the title/menu (player
                // ABSENT); a genuine switch arms in-world (player PRESENT). So the time-based disarm now fires
                // only when SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT==1 -- it kills the spurious boot self-reload
                // (latching load #1 DONE via phase IDLE, which gates OFF both this destructive branch and the
                // return-title chain submit) and never touches a real switch. Reset the counter whenever
                // nothing is armed so only *continuous* armed presence counts. The completed-switch success
                // latch (recognising a genuine switch's NEW stable world so the DLL stops re-driving) is
                // handled separately by the in-world stable-load proof, not by this disarm.
                // SLOT-AWARE-BY-CAUSE discriminator (2026-07-18, supersedes the pure time-based gate).
                // Only the SPURIOUS boot self-reload is disarmed: it is armed while the player is ABSENT
                // (the boot autoload's own ProfileSelect navigation queuing a post-load reload of the very
                // character it is loading). A GENUINE in-world switch arms with the player PRESENT and must
                // be left to run its return-title teardown+reload -- disarming it by elapsed time is the
                // switch-regression (bd angre-4loads-goal-met-but-switch-regression-2026-07-18), where the
                // old world lingers past the threshold and the switch gets cancelled ("world resolves and
                // I'm still on the old character"). Gating on SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT keeps load #1
                // stable (kills the spurious arm) without touching real switches. See
                // bd repeatable-multi-save-consolidated-plan-2026-07-18.
                if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                    >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
                    && SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT.load(Ordering::SeqCst) == 1
                {
                    let armed = SYSTEM_QUIT_INWORLD_ARMED_STABLE_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
                    if armed == SYSTEM_QUIT_INWORLD_ARMED_DISARM_TICKS {
                        SYSTEM_QUIT_QUICKLOAD_PHASE
                            .store(SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE, Ordering::SeqCst);
                        SYSTEM_QUIT_INWORLD_ARMED_DISARM_COUNT.fetch_add(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: SPURIOUS boot self-reload arm (armed while player absent) DISARMED after {armed} continuous in-world frames -> phase IDLE; destructive reload suppressed (genuine in-world switches are NOT disarmed)"
                        ));
                    }
                } else {
                    SYSTEM_QUIT_INWORLD_ARMED_STABLE_TICKS.store(0, Ordering::SeqCst);
                }
                // MENU-FREE RELOAD COMPLETION LATCH (2026-07-18, repeatability fix, bd
                // repeatability-menu-free-phase-reset-fix-2026-07-18). own_load_switch_reload_fire committed
                // the picked slot (FRESH_DESER_DONE=1) and its native SetState5 began streaming the new
                // character, but the switch phase is still armed. Left armed after the load is genuinely
                // playable, the return-title branch can keep re-driving state that belongs to the next switch.
                // FRESH_DESER_DONE is only a deserialize/SetState5 handoff proof, NOT a playable-world
                // proof. The driver now owns the stricter per-epoch movement/native-settle gate before it may
                // start another switch. This latch has a different job: disarm product_core_autoload_tick as
                // soon as the native MoveMap child is done so title-loop ownership does not take the loaded
                // player back down during AUTOLOAD_HANDOFF. For strict repro probes, however, native MoveMap
                // completion alone is not enough: the known bug is exactly "MoveMap finished, then requestCode
                // drains and the player disappears before movement." Keep handoff armed until the current reload
                // epoch has epoch-scoped movement proof too. Normal user sessions keep the original non-input
                // player-present latch and are not forced to walk the character.
                // DE-GATED (deprecate-env-marker-gate-allowlists-2026-07-19): marker feature gates are
                // forbidden; the movement-proof harness marker is retired, so no epoch is ever forced
                // to walk the character (proof-only behavior, never product).
                let movement_proof_required = false;
                let current_epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
                let movement_proven_for_epoch = CAN_MOVE_CONFIRMED.load(Ordering::SeqCst)
                    && MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == current_epoch;
                let native_movemap_child_done = if movement_proof_required {
                    let owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
                    let ingame = if owner != TITLE_OWNER_SCAN_START_ADDRESS && owner > 0x10000 {
                        unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
                            .filter(|ig| *ig != TITLE_OWNER_SCAN_START_ADDRESS && *ig > 0x10000)
                    } else {
                        None
                    };
                    ingame
                        .and_then(|ig| unsafe { safe_read_usize(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) })
                        .filter(|mms| *mms != TITLE_OWNER_SCAN_START_ADDRESS && *mms > 0x10000)
                        .and_then(|mms| unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_RE_OFFSET) })
                        .unwrap_or(-1)
                        == -1
                } else {
                    true
                };
                let menu_free_reload_ready = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE
                    .load(Ordering::SeqCst)
                    == 1
                    && (!movement_proof_required
                        || (native_movemap_child_done && movement_proven_for_epoch));
                if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                    >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
                    && menu_free_reload_ready
                {
                    let stable =
                        SYSTEM_QUIT_MENU_FREE_STABLE_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
                    let required_stable = if movement_proof_required {
                        1
                    } else {
                        SYSTEM_QUIT_MENU_FREE_STABLE_TICKS_THRESHOLD
                    };
                    if stable == required_stable {
                        SYSTEM_QUIT_QUICKLOAD_PHASE
                            .store(SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE, Ordering::SeqCst);
                        if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
                            er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
                        }
                        append_autoload_debug(format_args!(
                            "menu-free reload COMPLETION: picked char stable in-world {stable} frames (FRESH_DESER_DONE=1 movement_required={movement_proof_required} movement_proven={movement_proven_for_epoch} native_movemap_child_done={native_movemap_child_done}) -> phase IDLE, cleared save_requested; native owns warp_requested autoclear; return-title chain disarmed so the loaded world persists for the next switch"
                        ));
                    }
                } else {
                    SYSTEM_QUIT_MENU_FREE_STABLE_TICKS.store(0, Ordering::SeqCst);
                }
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
                    PLAYER_CURRENT_ANIMATION_ID.store(0, Ordering::SeqCst);
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
                PLAYER_CURRENT_ANIMATION_ID.store(
                    observation.current_animation_id.unwrap_or(0),
                    Ordering::SeqCst,
                );
                if observation.current_animation_id == Some(APPEAR_ANIMATION_ID)
                    || observation.appear_newly_queued
                {
                    state.expected_animation_seen = true;
                }
                state.last_write_idx = Some(observation.write_idx);

                process_global_driver_command(&mut state);
                write_telemetry_throttled(&mut state, true);
            },
            CSTaskGroupIndex::FrameBegin,
        );
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_RECURRING_REGISTERED,
            BOOTSTRAP_DETAIL_DONE,
        );
        // LIVE LOADING PORTRAIT render/publish pump: register in each candidate DRAW phase so exactly
        // one active phase can run on the render thread inside a live GX frame. This keeps the portrait
        // visible/refreshing during loading; cursor/head tracking remains retired.
        let portrait_phases = [
            CSTaskGroupIndex::Draw_Pre,
            CSTaskGroupIndex::GraphicsStep,
            CSTaskGroupIndex::DrawStep,
            CSTaskGroupIndex::DrawBegin,
            CSTaskGroupIndex::GameSceneDraw,
            CSTaskGroupIndex::AdhocDraw,
            CSTaskGroupIndex::DrawEnd,
            CSTaskGroupIndex::Draw_Post,
        ];
        for (i, phase) in portrait_phases.into_iter().enumerate() {
            cs_task.run_recurring(
                move |task_data: &FD4TaskData| unsafe {
                    profile_lookat_phase_draw_tick(i, task_data)
                },
                phase,
            );
        }
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
                let _bt = std::time::Instant::now();
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
                er_telemetry::counters::BUILD_DRIVER_LAST_US
                    .store(_bt.elapsed().as_micros() as usize, Ordering::SeqCst);
            },
            CSTaskGroupIndex::FrameBegin,
        );
    });
}
