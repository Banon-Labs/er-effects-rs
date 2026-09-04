pub(crate) use er_telemetry_core::counters::AUTOLOAD_HANDOFF_PARENT_STATE_FIX_COUNT;

fn poll_cached_mms18_ending_request_advancer() {
    // Native full deserialize owns GameMan::warp_requested and MoveMapStep::CheckReturnToTitle
    // consumes/autoclears it at finalize case 8. Agent-side 0x5d or warp pulses finalize by tearing
    // down the loaded world. The only post-finalize cleanup we own is menuData+0x5e: native sets it
    // while walking 12a->case8, but leaves it true after mms leaves 18; if it remains true into the
    // resident world, the player is torn down about a second later.
    //
    // THE PHASE GATE IS NOT ENOUGH, AND THE RESIDUAL OUTLIVES IT (2026-09-04, black-screen run).
    //
    // `SYSTEM_QUIT_QUICKLOAD_PHASE` is driven to IDLE the instant the native slot deserialize is
    // proven -- `system-quit-quickload: native slot deserialize proof OK ... -> phase IDLE`, logged
    // at +2029210ms with `world_up=false`. The world then takes another ~14s to stream in. So on a
    // Load-Character-from-File switch this poll is already gated shut for the whole window in which
    // the residual actually bites: measured ZERO `ENDING-FLAG POST-FINALIZE CLEAR` lines in a 5.9 MB
    // log of a run that reverted at +2043697ms.
    //
    // `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE` is the durable "a switch reload committed"
    // latch (set at the feed/continue_confirm commit, cleared only when a NEW switch arms), so it
    // survives that phase reset and covers the same window the phase gate was meant to cover.
    //
    // The sibling clear in `er_title_flow::product_core_autoload_tick`
    // (`reload-ending-latch-residual-clear`) has the RE-correct CONDITION but is unreachable here:
    // that tick early-returns at its `title_owner()` gate, which is None during stable in-world.
    // Same run: `product_core_autoload_ticks=71` against `product_core_callsite_ticks=36814`, with
    // `product_core_ready_blocks=69` -- its ending-latch code ran at most twice all session.
    let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let switch_reload_committed =
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 1;
    if quickload_phase < SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
        && !switch_reload_committed
    {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let Some(md) = unsafe { safe_read_usize(er_game_base::mem::game_data_addr(base, CS_MENU_MAN_GLOBAL_RVA, "CS_MENU_MAN_GLOBAL_RVA")) }
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
    // WHY `warpRequested == 0` IS THE RIGHT SECOND CONDITION, AND WHY THE mms/requestCode SHAPE IS
    // NOT (RE er-effects-rs-9fmm, re-confirmed by the 2026-09-04 revert).
    //
    // RE-GROUNDED ON 1.17 (2026-09-04). The reading below was originally decompiled off the 1.16.2
    // dump on :8765; the INSTALLED game is 1.17. Every address here is now measured on the 1.17
    // dump (:8767, `proj1170`) and identity-checked against `eldenring-deobf-1.17.bin`
    // (`check-dump-deobf-identity.py` -> MATCH, shift 0). The 1.16.2 addresses are kept only as the
    // pairing evidence, never as current.
    //
    // The evaluator is `FUN_140afb9f0` on 1.17 (1.16.2: `FUN_140afa6d0`; the `FUN_140afa7c0` this
    // comment used to name is not a function entry on EITHER build -- it lands 0xf0 inside the
    // 1.16.2 one). Paired by the unique wide literal `L"CSEzSelectBot.MoveMapStep"` -- 1.16.2
    // `0x142b60758`, 1.17 `0x142b637f8` -- which both functions reference at the SAME +0xc1 from
    // entry, with the SAME body size (4491). The byte mapper cannot carry this address: it reports
    // UNRESOLVED (111 shape matches, none at the nearest anchor delta), so the literal is the proof.
    //
    // Measured on 1.17, at instruction level:
    //   * `MOV byte ptr [RAX + 0x5e], BL` @ `0x140afbd0c` -- the write, UNCONDITIONAL and ahead of
    //     the switch, so it happens on every call.
    //   * `MOVZX EAX, byte ptr [RAX + 0x5d]` -- +0x5d feeds the same disjunction it always did.
    //   * `RAX` is `*(CSMenuMan + 8)` (menuData); CSMenuMan global = `0x143d6f820` on 1.17.
    //   * `switch (*(param_1 + 0x12a)) { case 0: if (cVar == 0) return; ... }` -- the finalize
    //     driver, unchanged. `case 8` calls `FUN_14067bcf0(0)`, which is `*(GameMan + 0x10) = 0`:
    //     the warp really is consumed there.
    //   * `warpRequested` is read by `FUN_14067a660` = `return *(GameMan + 0x10)`, so
    //     `GAME_MAN_WARP_REQUESTED_10_OFFSET = 0x10` holds on 1.17.
    //
    // So while warpRequested==1 the flag is the live finalize driver and clearing it SABOTAGES the
    // finalize. Once case 8 has consumed the warp (warpRequested 1->0) nothing re-evaluates the
    // flag and it stays 1 RESIDUAL.
    //
    // That makes this a residual SCRUB, not a suppression: if any genuine end condition still holds,
    // the native evaluator re-asserts 0x5e on its very next frame. The residual is only reachable
    // BECAUSE the evaluator has stopped.
    //
    // WHAT THE SCRUB CANNOT RACE (measured on both builds, 2026-09-04). Scanning every RIP-relative
    // load of the CSMenuMan global (779 sites on each build) and decoding 100 instructions forward,
    // `menuData+0x5e` is touched by exactly TWO instructions per build -- one write and one read:
    //   1.16.2  write `0x140afa9ec` (evaluator)      read `0x140844023`
    //   1.17    write `0x140afbd0c` (evaluator)      read `0x140845013`
    // The evaluator is the ONLY writer, which is what makes a scrub safe: nothing else can be
    // mid-write, and the evaluator overwrites us next frame whenever it is still running.
    //
    // THE REVERT IS NOT CAUSED BY THIS FLAG. MEASURED, NOT INFERRED (2026-09-04).
    //
    // This comment used to claim "STEP_EndFlow reads that as return-to-title -> SetState(6 -> 2)".
    // That is false on both builds, and the real decider has now been read end to end.
    //
    // A `SetState(owner, 2=BeginLogo)` from committed=6 comes from `STEP_GameStepWait`
    // (1.16.2 `0x140b0cde0` / 1.17 `FUN_140b0e480`, both size 437, delta +0x16a0; the 1.17 one
    // identity-checked against the image, shift 0). Its ENTIRE decision is three reads:
    //
    //     if (InGameStep->requestCode_0xd8 == 0) {        // else: returns, no SetState at all
    //       if (GameMan+0xb7c == 0) {                     // else: state 7
    //         if (GameMan+0xb7d == 0) -> state 2 BeginLogo // else: state 9
    //
    // `menuData+0x5e` does not appear. `CS::TitleStep::STEP_EndFlow` (1.16.2 `0x140b0cc00`) never
    // references the byte either, and the ONLY reader of it anywhere reachable through the CSMenuMan
    // global is `AddEntry(SummonMsgQueue*, SummonMsgData*)` (1.16.2 `0x140843f70` /
    // 1.17 `0x140844f60`, both size 398, read at the same +0xa3), which merely refuses to enqueue a
    // summon message while the flag is set.
    //
    // WHY THE WRONG SUSPECT LOOKED GUILTY: the `title-setstate-trace` line that motivated the theory
    // logged `warp/b73/bc4/md5d/md5e` and NOT `b7c`/`b7d` -- it sampled the MoveMapStep evaluator's
    // inputs, not GameStepWait's. md5e was simply the only logged field that was set. That line now
    // also logs `GAMESTEPWAIT[req_d8, b7c, b7d]`, so the next run names the branch instead of us
    // guessing.
    //
    // CONSEQUENCE FOR THIS SCRUB: it is safe (single writer, re-asserted next frame while the
    // evaluator runs) but it is NOT the fix for the revert. To hold off BeginLogo during the
    // streaming window the lever is `InGameStep+0xd8 != 0` -- which skips the SetState branch
    // entirely -- not this byte. Left in place rather than ripped out, because it is harmless and
    // removing it is a behaviour change that deserves its own runtime run; do not cite it as the
    // black-screen fix.
    //
    // Scan caveat, unchanged: the reader search only sees code that reaches menuData THROUGH the
    // global, so a callee handed the pointer as an argument would be missed.
    //
    // The old `mms_step == -1 && request_code == MOVEMAP_PENDING` shape is exactly the class of
    // over-gating the sibling clear already had to delete: its own comment records "run 1707: the
    // clear fired 0 times because the mms>=18/player sub-gate excluded that frame". The 2026-09-04
    // revert frame reported `req_code=0(NONE)` (gm-snap `ig_d8` 2 -> 0 across +2042168..+2043687),
    // so that shape would have missed it too. Keep it only as an additional trigger, never as a
    // requirement.
    //
    // `md_5d == 0` stays the SAFETY discriminator and is what makes this poll safe to run always:
    // a genuine user/return-title request carries BOTH flags (measured: the intended switch teardown
    // at +2028671ms logged `md5d=1 md5e=1`, the spurious revert at +2043697ms logged `md5d=0
    // md5e=1`), so a real return-to-title is never scrubbed.
    let gm = game_man_ptr_or_null();
    let warp_requested = if gm > PAB_MIN_HEAP_PTR {
        unsafe { safe_read_u8(gm + GAME_MAN_WARP_REQUESTED_10_OFFSET) }
            .map(i32::from)
            .unwrap_or(1)
    } else {
        1
    };
    let warp_consumed = warp_requested == 0;
    let legacy_post_finalize =
        mms_step == -1 && request_code == INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING;
    // OBSERVE-ONLY SINCE 2026-09-04 -- THE WRITE IS GONE, DELIBERATELY. Read the next paragraph
    // before restoring it.
    //
    // The comment above concluded this scrub was "harmless" and left it in. Two live runs then
    // measured it firing 5ms (br-20260904-231726-2f1a, +64388 -> +64393) and 6ms
    // (br-20260904-181251-0586, +75610 -> +75616) before `MMS-CLEANUP: child leaving STEP_MoveMap ->
    // Cleanup`, and firing EXACTLY ONCE PER RUN -- only on the load that ends in the black screen,
    // never on the stable teardown earlier in the same session. "Harmless" is not supported by that.
    //
    // Its own trigger is the problem: `warpRequested == 0` becomes true the INSTANT case 8 of the
    // ending evaluator consumes the warp, which is INSIDE the finalize, not after it. The child
    // still has to reach its terminal. So the scrub does not clear a settled residual -- it writes
    // into the middle of a live finalize, on the one code path where the world is being streamed in.
    //
    // The counter is kept so the condition stays measurable: `ENDING_REQUEST_SET_COUNT` now means
    // "times the scrub WOULD have fired". If the black screen still reproduces with this count
    // non-zero, the byte is exonerated for good and the cause is elsewhere; if it stops, the write
    // was the cause. That is the whole point of removing it rather than gating it behind a flag.
    if warp_consumed || legacy_post_finalize {
        let n = ENDING_REQUEST_SET_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 8 || n.is_power_of_two() {
            append_autoload_debug(format_args!(
                "ENDING-FLAG POST-FINALIZE CLEAR #{n}: WOULD have cleared menuData+0x5e (write REMOVED 2026-09-04) -- phase={quickload_phase} switch_reload_committed={switch_reload_committed} warpRequested={warp_requested} requestCode={request_code} mms={mms_step}; the byte is left exactly as the native evaluator wrote it"
            ));
        }
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
        er_telemetry_core::counters::GAME_TASK_LAST_US
            .store(self.0.elapsed().as_micros() as usize, Ordering::SeqCst);
    }
}

pub(crate) fn spawn_game_task(state: Arc<Mutex<EffectsState>>) {
    std::thread::spawn(move || {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_THREAD_STARTED,
            BOOTSTRAP_DETAIL_START,
        );
        let Some(cs_task) = wait_for_task_instance() else {
            // The product without its per-frame task is a product that does nothing, which is
            // still strictly better than the alternative this replaced: a thread spinning on
            // `yield_now` hard enough to stop the game from ever reaching a window.
            append_autoload_debug(format_args!(
                "game task: CSTaskImp never appeared -- no per-frame tick registered; the DLL \
                 stays inert rather than spinning"
            ));
            return;
        };
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_INSTANCE_READY,
            BOOTSTRAP_DETAIL_DONE,
        );
        // Boot-phase marker: CSTaskImp resolved -> bounds the end of the pre-instance engine-init
        // gap (the largest uninstrumented boot window) in the same [+Nms] timeline the renderer parses.
        if er_boot_profiler::profiler_enabled() {
            append_autoload_debug(format_args!("boot-phase: cstask_instance_ready"));
        }

        cs_task.run_recurring(
            move |task_data: &FD4TaskData| {
                let _gt = GameTaskTimer(std::time::Instant::now());
                // Free-running, lock-free liveness beat. `EffectsState::game_task_ticks` counts the
                // same thing but only a telemetry write this task performs can publish it, so it
                // cannot answer "is this task still running" for anyone else. Any thread can read
                // this one, which is what lets the boot picker record whether the game was alive
                // across a dialog that blocked for half a minute.
                er_telemetry_core::counters::GAME_TASK_TICKS_TOTAL.fetch_add(1, Ordering::SeqCst);
                // Boot-phase marker: first frame our recurring task actually ticks.
                if er_boot_profiler::profiler_enabled()
                    && BOOT_FIRST_FRAME_LOGGED
                        .swap(GAME_TASK_TICK_INCREMENT as usize, Ordering::SeqCst)
                        == 0
                {
                    append_autoload_debug(format_args!("boot-phase: first_game_frame"));
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
                    // Answer the container question DllMain could not (bd er-effects-rs-1742).
                    // Idempotent after the first call; a no-op on every run that did not defer.
                    resolve_deferred_save_override();
                    // REPAIR THE PROFILE SUMMARY BEFORE THE TITLE BUILDS ITS CONTINUE ROW.
                    // The game deserializes `CS::ProfileSummary` exactly once per boot, and a boot
                    // whose save-data read completes with no data leaves all ten records zeroed
                    // with nothing to retry it. The title then inserts Continue through its
                    // DISABLED edge and never re-arms it: measured run 2026-09-05 21:29:58, the
                    // records were repaired at +13956ms but the row had been built at +11131ms, so
                    // the autoload still parked on "waiting for native Continue MenuWindowJob
                    // result" with 260/260 candidate observations carrying the idle accept
                    // predicate. Running the same repair from HERE -- the pre-player boot tick --
                    // puts real records in front of the row builder instead of behind it.
                    // Self-throttling, and a no-op once the records and the container agree.
                    refresh_boot_default_profile_summary();
                    // Read-only observer on the MoveMap ending-request evaluator. Installed from the
                    // BOOT tick, not from the switch path, because the black screen now reproduces on
                    // the first autoload: the world loads and is playable, then ~2.2 s after world
                    // entry this evaluator raises `menuData+0x5e` and the session ends. Idempotent.
                    er_title_flow::install_movemap_advancer_probe();
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
                    if let Ok(base) = game_module_base() {
                        unsafe { profile_editor_necromancy_tick(base) };
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
                            // DELIBERATE BEHAVIOR CHANGE (bd er-effects-rs-91zb; IMPLEMENTED BUT
                            // UNPROVEN -- no live run has exercised it). The missing-save picker
                            // cannot set a config slot; its character sub-picker records the chosen
                            // slot here. This used to be `autoload.slot().or_else(picker)` --
                            // "configured slots still win" -- while the OTHER resolver for the same
                            // question, `continue_load::slot_resolution::native_fullread_slot`,
                            // puts the picker FIRST. Two resolvers disagreeing about which slot the
                            // user meant is how a portrait ends up targeting one character while
                            // another loads. Resolved in the picker's favour on both sides: a
                            // user's explicit on-screen pick outranks a config default, which is a
                            // stale preference by comparison.
                            missing_save_picker_selected_slot()
                                .or_else(|| state.autoload.slot())
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
                    // OWN-STEPPER installs the idx10 patch so OUR handler runs each frame.
                    if own_stepper_enabled() || native_continue_enabled() || own_load_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { own_stepper_patch_once(base) };
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
                    process_safe_input_request(&mut state);
                    process_autoload_request(&mut state);
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
                // The old OPEN_MENU..CONFIRM menu-nav exclusion is GONE: those autopilot states no longer
                // exist. `system_quit_repro_tick` now runs WAIT_WORLD -> WAIT_RELOAD -> DONE only, and all
                // three are in-world settle states where the probe MUST run (that is where load1 and each
                // reload prove movement). Nothing injects a menu cursor any more, so there is nothing to
                // exclude.
                // Only inject once the char is actually RENDERED in-world (render_group 1c4 + enable_render
                // 1c5), NOT merely present. `player present` goes true mid-load (mms=13, ~14s before
                // render_group), and injecting there latched an invalid DISPROVEN before the char could be
                // controllable -- then the verdict was frozen and never re-tested (run 092119: verdict at
                // t=79.9s during loading; render_group did not fire until t=86s). Gating on the rendered
                // state makes the probe test movability at the stable in-world point, so a DISPROVEN verdict
                // means the char genuinely did not move, not that we injected before the world was up.
                let char_rendered = player.chr_ins.chr_flags1c4.is_render_group_enabled()
                    && player.chr_ins.chr_flags1c5.enable_render();
                if char_rendered {
                    let p = player.chr_ins.modules.physics.position;
                    crate::experiments::can_move_probe::tick((p.0, p.1, p.2));
                }
                // NO PROGRAMMATIC SWITCH TRIGGER LIVES HERE ANY MORE (deleted 2026-09-05, user
                // directive). `poll_switch_slot_control_file` used to read a game-directory control
                // file and arm a MENU-FREE character switch straight into
                // `own_load_switch_reload_fire`, so every second and third load this project ever
                // measured skipped the Quit-menu path a real player has to take. That made it the
                // wrong instrument for the only thing it was used for: proving the menu flow works.
                // A second load must now come from the menu, or not at all.
                poll_cached_mms18_ending_request_advancer();
                if let Ok(base) = game_module_base() {
                    unsafe {
                        profile_editor_necromancy_tick(base);
                    }
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
                if (own_stepper_enabled() || native_continue_enabled())
                    && let Ok(base) = game_module_base() {
                        unsafe {
                            cleanup_title_dialog_after_world_once(base, state.game_task_ticks)
                        };
                    }
                // In-world correctness oracle: on the FIRST frame the local player exists, log
                // the load-correctness record + the T_controllable timeline marker ONCE. Fires
                // for both a native-menu load (observe) and a DLL-driven load (own-stepper), so
                // the two records are directly comparable (field-for-field == correct load).
                if (own_stepper_enabled() || native_continue_enabled())
                    && LOAD_CORRECTNESS_DUMPED
                        .swap(GAME_TASK_TICK_INCREMENT as usize, Ordering::SeqCst)
                        == LOAD_CORRECTNESS_NOT_DUMPED
                    && let Ok(base) = game_module_base() {
                        timeline_event(
                            "T_controllable",
                            state.game_task_ticks,
                            format_args!("player=1"),
                        );
                        unsafe { dump_load_correctness(base, state.game_task_ticks) };
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
        // BUILD IMPORT (System>Quit "Load Build from URL"). FrameBegin is the game thread, which is
        // what every step of the import needs -- it mutates the inventory, `CSGaitemImp`,
        // `PlayerGameData` and the equipment slots through the game's own functions. Registered
        // unconditionally and from boot because it is inert until a row press queues a build: the
        // runtime's tick returns immediately unless its phase is `Ready`, so the cost of an idle
        // frame is one atomic load.
        cs_task.run_recurring(
            move |_task_data: &FD4TaskData| {
                // Safety: FrameBegin runs on the game task thread, the context the runtime requires;
                // every step inside it is individually precondition-checked (params streamed,
                // character present).
                unsafe { system_quit_build_import_tick() };
            },
            CSTaskGroupIndex::FrameBegin,
        );
        // BUILD EXPORT (System>Quit "Generate Build Link"). Same thread and the same reason, from
        // the other direction: this one READS `PlayerGameData`, the equipment slots and the message
        // repository, none of which may be touched off the game thread. Also inert until pressed --
        // and it deliberately ticks even when idle, because its tick counter is the witness the
        // stale-latch check measures against (see `er_build_import_runtime::export`).
        cs_task.run_recurring(
            move |_task_data: &FD4TaskData| {
                // Safety: FrameBegin runs on the game task thread; every step inside is
                // precondition-checked (params streamed, character present).
                unsafe { system_quit_build_export_tick() };
            },
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
                er_telemetry_core::counters::BUILD_DRIVER_LAST_US
                    .store(_bt.elapsed().as_micros() as usize, Ordering::SeqCst);
            },
            CSTaskGroupIndex::FrameBegin,
        );
    });
}
