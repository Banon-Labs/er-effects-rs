use super::*;

/// Cumulative ProfileSelect OK-confirm count (cancel-close BLOCK + ALLOW). Legacy fallback signal:
/// the CONFIRM state's primary advance is the direct-arm phase observation; this count (an INCREASE
/// over the per-switch baseline, so switch #2 does not trip on switch #1's residual) only fires if
/// the pick fell through to the native confirm-box -> OK -> load-job chain.
pub(crate) fn sq_repro_confirm_count() -> usize {
    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.load(Ordering::SeqCst)
        + SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.load(Ordering::SeqCst)
}

pub(crate) fn sq_repro_native_load_state() -> (i32, i32, bool) {
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
    let mms_live = ingame
        .and_then(|ig| unsafe { safe_read_usize(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) })
        .filter(|mms| *mms != TITLE_OWNER_SCAN_START_ADDRESS && *mms > 0x10000)
        .and_then(|mms| unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_RE_OFFSET) })
        .unwrap_or(-1);
    let native_load_settled =
        request_code == INGAMESTEP_REQUEST_CODE_STABLE_IN_WORLD && mms_live == -1;
    (request_code, mms_live, native_load_settled)
}

/// The ProfileSelect slot the current switch loads (clamped to the target table).
///
/// Runtime override so the multi-load proof harness can drive an explicit sequence of DISTINCT
/// within-file characters without a recompile: a game-dir CONTROL FILE
/// `er-effects-sq-target-slots.txt` = comma/space-separated slot indices (e.g. "0,2,3"). Switch i
/// loads the i-th entry (clamped to the last). Absent/unparseable -> the compile-time
/// SQ_REPRO_TARGET_SLOTS table. Control file, not an env gate (env gates are frozen).
pub(crate) fn sq_repro_target_slot() -> i32 {
    let i = SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst);
    let path = game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-sq-target-slots.txt");
    if let Ok(contents) = std::fs::read_to_string(&path) {
        let slots: Vec<i32> = contents
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter_map(|t| t.trim().parse::<i32>().ok())
            .collect();
        if !slots.is_empty() {
            return slots[i.min(slots.len() - 1)];
        }
    }
    SQ_REPRO_TARGET_SLOTS[i.min(SQ_REPRO_TARGET_SLOTS.len() - 1)]
}

/// How many back-to-back switches to drive in the legacy ProfileSelect harness path. The active Save
/// Game row validation path below is always-on and no longer reads an env selector.
pub(crate) fn sq_repro_target_switches() -> usize {
    // Runtime override so the multi-load proof harness can drive N back-to-back switches without a
    // recompile (diagnostic/harness knob only; the shipped default stays SQ_REPRO_TARGET_SWITCHES=1).
    // Uses a game-dir CONTROL FILE (`er-effects-sq-target-switches.txt` = a decimal count), the same
    // sanctioned runtime-marker pattern as the other sq-repro modes -- NOT a new ER_EFFECTS_* env gate
    // (env gates are frozen by .auto/env_gate_comment_policy.rego). Absent/unparseable -> the const.
    let runtime = game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-sq-target-switches.txt");
    std::fs::read_to_string(&runtime)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(SQ_REPRO_TARGET_SWITCHES)
        .clamp(0, SQ_REPRO_TARGET_SLOTS.len())
}

/// Legacy pause-at-menu mode is disabled while the always-on Save Game row repro is active.
pub(crate) fn sq_repro_pause_at_menu() -> bool {
    false
}

/// PROFILE-LOAD-SWITCH repro mode: drive the user's exact switch (Quit tab -> Load Profile ->
/// ProfileSelect -> pick the top character -> confirm -> load). Gated by its OWN marker
/// `er-effects-system-quit-load-switch.txt` / `ER_EFFECTS_SQ_LOAD_SWITCH=1` -- deliberately NOT the
/// `system_quit_profile_load_activation_allowed` opt-in, because that opt-in makes the ProfileSelect
/// slot-activate hook FORWARD to the guarded native load instead of DIRECT-ARMING the save-safe switch
/// (system_quit_ownership_repro.rs:1215 gates the direct-arm on `!allowed`). The direct-arm is exactly
/// what the switch needs (it sets QUICKLOAD_PHASE and drives return-title + reload), so this mode must
/// leave activation-allowed OFF.
// ENV-GATE RATIONALE: ER_EFFECTS_SQ_LOAD_SWITCH=1 selects the profile-load-switch repro autopilot (Quit
// tab -> Load Profile -> pick top character -> confirm -> load). This mode DOES drive a real profile
// load/reload; agent-owned repro only, gated separately from the default Save Game harness.
#[allow(dead_code)] // Retained: Load-switch repro mode selector; the presence-gate contract in its doc is the retained part.
pub(crate) fn sq_repro_load_switch_mode() -> bool {
    // DECOUPLED TOGGLE (2026-07-19): the load2 flow drives a REAL profile-load switch (Quit tab ->
    // Load Profile -> pick top character -> confirm -> load) whenever the separate input-harness DLL
    // is loaded in the profile. Presence-gated (GetModuleHandle), not env/marker. bd
    // harness-orchestrates-product-exposes-primitives-boundary-2026-07-19.
    harness_dll_present()
}

/// Enter a switch: capture the confirm-count baseline and clear the per-switch menu-window/cursor
/// signals so the state machine re-detects them fresh for this switch (they hold stale pointers from
/// the prior switch otherwise). Called before OPEN_MENU for every switch.
pub(crate) fn sq_repro_begin_switch() {
    SQ_REPRO_CONFIRM_BASELINE.store(sq_repro_confirm_count(), Ordering::SeqCst);
    SYSTEM_QUIT_INGAME_TOP_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_OPTION_SETTING_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.store(0, Ordering::SeqCst);
    SQ_REPRO_INITIAL_CURSOR.store(usize::MAX, Ordering::SeqCst);
    SQ_REPRO_WAIT_RELOAD_FRAMES.store(0, Ordering::SeqCst);
    // TO_PROFILE one-shot latches: reset PER SWITCH so switch #2+ re-builds the Quit-tab pane, re-captures
    // a FRESH Load-Profile action, and re-fires the deterministic route -- the exact path that worked for
    // switch #1. WITHOUT this, these stay latched from switch #1 and switch #2 skips both the pane-rebuild
    // and the route-fire, falling through to the mouse-only tab-switch key probe that never succeeds
    // (observed run samechar-3x-190909: switch #2 stuck in TO_PROFILE, load3 never attempted). Also clear
    // the captured action so TO_PROFILE sees route_action==0 -> rebuilds -> fresh capture (never fires a
    // stale/freed switch-#1 action), and reset the tab-discovery + row-nav bases for the fallback path.
    SQ_REPRO_PANE_BUILD_TRIED.store(0, Ordering::SeqCst);
    SQ_REPRO_ROUTE_FIRED.store(0, Ordering::SeqCst);
    SQ_REPRO_TAB_DISCOVERED.store(0, Ordering::SeqCst);
    SQ_REPRO_TAB_BASELINE.store(usize::MAX, Ordering::SeqCst);
    SQ_REPRO_ROWNAV_BASE.store(usize::MAX, Ordering::SeqCst);
    SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.store(0, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_OPENED.store(0, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_DONE.store(0, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_RESTORE_BASELINE.store(
        SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    SQ_REPRO_PROFILE_BACK_RESTORE_COUNT.store(0, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_FINAL_TAB.store(usize::MAX, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_BASELINE_MASK.store(0, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_VERIFY_MASK.store(0, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_MISMATCH_MASK.store(0, Ordering::SeqCst);
    for slot in &SQ_REPRO_PROFILE_BACK_BASELINE_HASHES {
        slot.store(0, Ordering::SeqCst);
    }
    for slot in &SQ_REPRO_PROFILE_BACK_BASELINE_COUNTS {
        slot.store(usize::MAX, Ordering::SeqCst);
    }
    for slot in &SQ_REPRO_PROFILE_BACK_VERIFY_HASHES {
        slot.store(0, Ordering::SeqCst);
    }
    for slot in &SQ_REPRO_PROFILE_BACK_VERIFY_COUNTS {
        slot.store(usize::MAX, Ordering::SeqCst);
    }
    // GX command-queue growth curve: log the finished switch's occupancy high-water + top
    // producers, then reset the per-switch high-water so each switch reports its own peak (the
    // 0x1aeaf05 overflow shows as this peak climbing toward cap across switches).
    let switch_peak = GX_CMD_QUEUE_SWITCH_MAX_FILL.swap(0, Ordering::SeqCst);
    let switch_arena_min = GX_CMD_ARENA_SWITCH_MIN_REMAINING.swap(usize::MAX, Ordering::SeqCst);
    GX_CMD_QUEUE_PEAK_LAST_LOGGED.store(0, Ordering::SeqCst);
    let (dd_pending, dd_highwater) = unsafe { delay_delete_pending() }
        .map(|(p, h)| (p as i64, h as i64))
        .unwrap_or((-1, -1));
    // Ownership-conservation check: if any native-owned class is over its bound, this is where the
    // spared-renderer leak would have surfaced (switch #2), long before the GX queue overflow crash.
    ownership_ledger_check("switch-boundary");
    append_autoload_debug(format_args!(
        "gx-cmdqueue: switch boundary -- prev-switch peak {switch_peak}/{} arena_min_remaining={} delaydelete_pending={dd_pending} (highwater {dd_highwater}) spared_outstanding={} ledger_violations={} (cumulative max {}, arena min {}, reserves {}) top producers: {} | buckets: {}",
        GX_CMD_QUEUE_CAP_SEEN.load(Ordering::SeqCst),
        fmt_lowwater(switch_arena_min),
        ownership_outstanding(OwnedClass::SparedRenderer),
        OWNED_LEDGER_VIOLATIONS.load(Ordering::SeqCst),
        GX_CMD_QUEUE_MAX_FILL.load(Ordering::SeqCst),
        fmt_lowwater(GX_CMD_ARENA_MIN_REMAINING.load(Ordering::SeqCst)),
        GX_CMD_QUEUE_SUBMITS.load(Ordering::SeqCst),
        gx_cmd_queue_hist_top(8),
        gx_cmd_queue_bucket_summary()
    ));
}

/// KEYBOARD MIRROR (native Windows): map a fabricated gamepad wButtons value to the equivalent DInput
/// DIK scancode. Runtime-proven 2026-07-17 that native ER polls XInput slot 0 (poll count climbed to
/// 7000+) and reads our fabricated START (4 poll-frames) but does NOT route fabricated gamepad input
/// to menu ACTIONS when no real controller is the active input device -- so the menu never opened. The
/// keyboard IS the active device on native, and ER routes DInput keyboard to menu actions (proven by
/// the `inject_nav` DIK_DOWN cursor move + the arrow-suppression hook). So the autopilot also stamps
/// the equivalent key each frame via `InputBlocker::set_injected_key`. Menu nav (arrows), Esc (open/
/// back), and Enter (confirm) are FIXED menu keys in ER -- not the remappable gameplay binds -- so
/// this is robust to the user's custom keybinds. Tab-switch (LB/RB) keyboard binds are the least
/// certain and are refined empirically from the phase the run stalls at. 0 = no key this frame.
pub(crate) fn sq_repro_gamepad_to_dik(btn: u16) -> u8 {
    match btn {
        XINPUT_GAMEPAD_START => 0x01, // DIK_ESCAPE  -- open the in-world system/escape menu
        XINPUT_GAMEPAD_A => 0x1c,     // DIK_RETURN  -- menu confirm/select
        XINPUT_GAMEPAD_B => 0x0e,     // DIK_BACK (Backspace) -- menu cancel/back
        XINPUT_GAMEPAD_DPAD_UP => 0xc8, // DIK_UP
        XINPUT_GAMEPAD_DPAD_DOWN => 0xd0, // DIK_DOWN
        XINPUT_GAMEPAD_LEFT_SHOULDER => 0x10, // DIK_Q  -- tab left (empirical; refine if TO_PROFILE stalls)
        XINPUT_GAMEPAD_RIGHT_SHOULDER => 0x12, // DIK_E  -- tab right (empirical)
        _ => 0,
    }
}

/// Native-Windows keyboard channel: map a fabricated gamepad button to a Win32 VK code posted to the
/// ER window via WM_KEYDOWN/WM_KEYUP. Runtime-proven 2026-07-17 that native ER reads keyboard via
/// window messages / RawInput (NOT DInput: dinput_kb_fires==0) and does not route the fabricated pad
/// to menu actions -- so this WM path is the actual driver on native; the XInput/DInk mirrors stay for
/// Wine. Esc opens the in-world menu; arrows nav; Enter confirms -- fixed menu keys, not the
/// remappable gameplay binds. Tab-switch (LB/RB) VKs are the least certain and refined empirically.
pub(crate) fn sq_repro_gamepad_to_vk(btn: u16) -> u32 {
    match btn {
        XINPUT_GAMEPAD_START => 0x1b, // VK_ESCAPE -- open the in-world system/escape menu
        XINPUT_GAMEPAD_A => 0x0d,     // VK_RETURN -- menu confirm
        XINPUT_GAMEPAD_B => 0x1b,     // VK_ESCAPE -- back (unused in the load path)
        XINPUT_GAMEPAD_DPAD_UP => 0x26, // VK_UP
        XINPUT_GAMEPAD_DPAD_DOWN => 0x28, // VK_DOWN
        XINPUT_GAMEPAD_LEFT_SHOULDER => 0x51, // 'Q' -- tab left (empirical; refine if TO_PROFILE stalls)
        XINPUT_GAMEPAD_RIGHT_SHOULDER => 0x45, // 'E' -- tab right (empirical)
        _ => 0,
    }
}

/// One-shot: we force-built the OptionSetting Quit-tab pane (via the game's own tab-select
/// FUN_14093b850) so the cloned Load-Profile row + its action object get created without the
/// mouse-only tab visit.
pub(crate) use er_telemetry::counters::SQ_REPRO_PANE_BUILD_TRIED;
/// One-shot: the deterministic Load-Profile route (`system_quit_open_profile_load_dialog`) was fired
/// this switch, so we do not re-fire it. Native ER has no keyboard bind for the OptionSetting
/// tab-switch (mouse-only), so instead of navigating to the Quit tab we invoke the DLL's own route
/// directly when the Load-Profile row action was captured -- it opens ProfileSelect and sets the
/// return-chain System dialog, exactly like a click.
pub(crate) use er_telemetry::counters::SQ_REPRO_ROUTE_FIRED;
pub(crate) use er_telemetry::counters::SQ_REPRO_ROWNAV_BASE;
pub(crate) use er_telemetry::counters::SQ_REPRO_TAB_BASELINE;
/// TO_PROFILE tab-switch key auto-discovery state (native keyboard): the discovered VK that moves
/// OPTIONSETTING_CURRENT_TAB (0 = not found yet), the tab index observed when the current candidate
/// window began (to detect a change), and the phase-local tick when we reached the Quit tab (so the
/// DOWN,DOWN,Enter row nav has its own base; usize::MAX = not yet on the Quit tab).
pub(crate) use er_telemetry::counters::SQ_REPRO_TAB_DISCOVERED;

/// SELF-DRIVEN System->Quit->Load-Profile REPRO AUTOPILOT tick (gated by `system_quit_repro_enabled`).
/// Runs every game-task frame. The input block stays engaged in-world (see `block_input_enabled`) so
/// the fabricated gamepad is the ONLY input and no human press can contaminate the repro. Drives the
/// user's EXACT Xbox controller sequence by writing `SQ_REPRO_XINPUT_BUTTONS` (read by the XInput
/// poll hook -- the stage the game reads a gamepad from), advancing ONLY on observed menu-window /
/// cursor / activate transitions (never timers or tap budgets):
///   START -> IngameTop; UP,A -> OptionSetting; LB,DOWN,DOWN,A -> ProfileSelect; one DOWN/UP off the
///   current save; A,A -> load armed -> DONE (block released; native pump drives return-title +
///   reload). Each phase issues its KNOWN edges once then HOLDS; a genuinely missed edge self-
///   reports (stuck waiting) instead of being papered over by a re-tap.
pub(crate) unsafe fn system_quit_repro_tick() {
    if !system_quit_repro_enabled() {
        return;
    }
    let state = SQ_REPRO_STATE.load(Ordering::SeqCst);
    if state == SQ_REPRO_STATE_DONE {
        return;
    }
    // Clear any stale keyboard stamp; `set_pad` below re-stamps the mirrored key for button frames.
    crate::input_blocker::InputBlocker::get_instance().set_injected_key(DIK_NONE);
    // Drive BOTH the XInput poll hook (SQ_REPRO_XINPUT_BUTTONS) AND the DInput keyboard mirror
    // (set_injected_key): native ER polls the pad but only routes KEYBOARD to menu actions when no
    // real controller is active (see sq_repro_gamepad_to_dik). One `set_pad` call drives both paths.
    let set_pad = |b: u16| {
        SQ_REPRO_XINPUT_BUTTONS.store(b as usize, Ordering::SeqCst);
        crate::input_blocker::InputBlocker::get_instance()
            .set_injected_key(sq_repro_gamepad_to_dik(b));
        // NATIVE path: post the equivalent VK to the ER window (keyboard is WM/RawInput, not DInput).
        sq_repro_drive_wm_key(sq_repro_gamepad_to_vk(b));
    };
    let tick = SQ_REPRO_STATE_TICK.fetch_add(1, Ordering::SeqCst);

    match state {
        SQ_REPRO_STATE_WAIT_WORLD => {
            set_pad(0);
            // Wait on the RELIABLE game-global readiness semaphore, NOT the flaky IN_WORLD_REACHED latch
            // (user 2026-07-21: the harness is flaky because it waits on a semaphore that may be wrong --
            // run 073411 never latched IN_WORLD_REACHED so no load2; 074159 did). BOOT_VIEW_EPOCH_WORLD_LIVE
            // == this fresh_deser epoch means the world clock is advancing for the CURRENT load = genuinely
            // in-world (set by the play_time_live oracle from GameDataMan+0xa0). The +180-tick settle below
            // still gates the actual menu-open, so an early world-live cannot fire the menu prematurely.
            let cur_epoch = crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT
                .load(Ordering::SeqCst);
            // FULL vanilla movable signature (user 2026-07-21: play_time_live ALONE armed the switch DURING
            // load1's loading -> soft-locked load1 mid-finalize with mismatched stats). Require the player
            // present + render-group enabled + enable_render (the char is genuinely rendered in-world) AND
            // the world clock live for THIS epoch -- the same gate WAIT_RELOAD uses. Only then is it safe to
            // arm the switch (return-title teardown), i.e. after load1 has actually finished loading.
            let render_ready = match unsafe { PlayerIns::local_player_mut() } {
                Ok(player) => {
                    player.chr_ins.chr_model_ins.as_ptr() as usize != TITLE_OWNER_SCAN_START_ADDRESS
                        && player.chr_ins.chr_flags1c4.is_render_group_enabled()
                        && player.chr_ins.chr_flags1c5.enable_render()
                }
                Err(_) => false,
            };
            let in_world = render_ready
                && crate::constants::BOOT_VIEW_EPOCH_WORLD_LIVE.load(Ordering::SeqCst) == cur_epoch;
            // PROGRAMMATIC SWITCH (user 2026-07-21): replace the OPEN_MENU->TO_SYSTEM->TO_PROFILE->TO_SLOT
            // ->CONFIRM SendInput menu drive (which forced ER foreground and stole the user's focus the
            // whole time) with the MENU-FREE arm. Once the CURRENT load's world is live + settled, arm the
            // switch directly: switch_slot_arm_programmatic sets the target slot + writes the return-title
            // teardown (menuData+0x5d=1); the game tears the old world down to a clean title (player
            // absent), then own_load_switch_reload_fire runs the native load (submit/drain/deser) +
            // continue_confirm -> SetState5 -- exactly like a vanilla Continue with the slot preselected.
            // No move-proof gate (it is foreground-limited and never fires); world-live+settle is the
            // reliable readiness. bd CORRECT-disable-custom-onclick-load-save-set-slot-then-native-continue.
            // Wait for load1 to PROVE movement (HARNESS_MOVE_VERDICT==1: the can-move probe confirmed
            // genuine injected-stick movement for this epoch) before arming the switch, so load1's
            // movement is proven -- not just render-ready. A timeout is a failed boot epoch; it must not
            // silently arm switch #1 and turn an unproven run into apparent multi-load evidence.
            let move_verdict_proven =
                crate::constants::HARNESS_MOVE_VERDICT.load(Ordering::SeqCst) == 1;
            if in_world && tick >= SQ_REPRO_WORLD_SETTLE_TICKS && move_verdict_proven {
                if sq_repro_pause_at_menu() {
                    // Diagnostic mode: 0 switches, no load. Nothing to drive without the menu-nav.
                    sq_repro_transition(SQ_REPRO_STATE_DONE);
                } else if let Ok(base) = game_rva(0) {
                    sq_repro_begin_switch();
                    let slot = sq_repro_target_slot();
                    unsafe { crate::experiments::switch_slot_arm_programmatic(base, slot) };
                    append_autoload_debug(format_args!(
                        "sq-repro: world-live+settled -> PROGRAMMATIC arm switch #{}/{} target_slot={slot} (menu-free, NO focus-steal); WAIT_RELOAD",
                        SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst) + 1,
                        sq_repro_target_switches(),
                    ));
                    sq_repro_transition(SQ_REPRO_STATE_WAIT_RELOAD);
                }
            } else if !in_world {
                // Not in-world yet (boot autoload still loading): hold the settle counter at 0.
                SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
            } else if tick == SQ_REPRO_MOVE_PROOF_TIMEOUT_TICKS {
                append_autoload_debug(format_args!(
                    "sq-repro: boot world is rendered but movement did not prove by {SQ_REPRO_MOVE_PROOF_TIMEOUT_TICKS}f -- NOT arming switch #1; preserving the failed boot epoch for the oracle"
                ));
            }
        }
        SQ_REPRO_STATE_WAIT_RELOAD => {
            // Between two back-to-back switches. Hold neutral while THIS switch's reload runs
            // (return-title tears down the old world, clean-title continue_confirm drives the fresh
            // picked-slot deserialize, SetState5 streams the new world). Advance to the next switch
            // only once the reload has COMMITTED (fresh-deser count reached this switch's number) AND
            // the NEW world is up (local player present) AND it has settled. Settle is counted from
            // the moment both hold (tick reset while the world is still down/loading), so it settles
            // the NEW world, not the residual old one.
            set_pad(0);
            let switch_index = SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst);
            let expected_deser = switch_index + 1;
            let deser = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
            // RENDER-READY gate (2026-07-18, user-directed): "present" is not "ready". The user's load-2
            // freeze is present-but-not-rendered, so gate the READY advance on the SAME render-ready
            // combination the oracle emits (chr-model present AND draw-group AND render-group AND
            // enable-render); a load stuck below it past the deadline is classified FROZEN below.
            let (player_up, render_ready) = match unsafe { PlayerIns::local_player_mut() } {
                Ok(player) => {
                    let model_present = player.chr_ins.chr_model_ins.as_ptr() as usize
                        != TITLE_OWNER_SCAN_START_ADDRESS;
                    // draw_group_enabled() is DROPPED -- it is NEVER set, even on a vanilla native
                    // Continue (verified run 074159 + vanilla-174933: chr_draw_group_enabled=NEVER), so
                    // requiring it hung load3 forever (user 2026-07-21: the harness waited on a semaphore
                    // that is simply wrong). render-group + enable-render is the reliable rendered signal.
                    let ready = model_present
                        && player.chr_ins.chr_flags1c4.is_render_group_enabled()
                        && player.chr_ins.chr_flags1c5.enable_render();
                    (true, ready)
                }
                Err(_) => (false, false),
            };
            // PlayerIns-present alone is a LOADING-SCREEN/TITLE FALSE POSITIVE (PlayerIns exists during
            // the reload before the world is interactive). Require the reload committed (fresh-deser),
            // the player present, AND the new load COMPLETE.
            //
            // STALL FIX (2026-07-03, autostep10 run: switch #1 hung here 21 min): `now_loading_active`
            // was used with INVERTED polarity. Despite its name it is a load-COMPLETE latch (RE-corrected
            // 2026-07-02): it reads FALSE while the map streams and flips TRUE when the load finishes,
            // then LINGERS true in gameplay. The old gate treated now_loading==true as "still on a loading
            // screen" and held -- so the instant switch #1's load completed (latch true) it hung forever.
            // Correct polarity (matches composite_portrait_inner's `loading = !load_done`): the world is
            // still loading while the latch is FALSE, done when it is TRUE. Advance only when the latch is
            // TRUE (load done), the fresh-deser count reached this switch, the player is up, and the cover
            // is gone. The lingering-true-from-the-previous-load risk is covered by fresh_deser (must
            // reach THIS switch's count) plus the settle wait below.
            let base = game_rva(0).unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let base_ok = base != TITLE_OWNER_SCAN_START_ADDRESS;
            let load_done = base_ok && unsafe { now_loading_active(base) };
            let fake_cover = base_ok && unsafe { fake_loading_screen_visible(base) };
            let _loading = !base_ok || !load_done || fake_cover;
            // READINESS GATE (2026-07-18, user-directed). "committed + player-present + cover-gone" is
            // NOT enough -- it holds during the load-2 freeze. Require RENDER-READY held for the settle
            // window before advancing. Hold the settle clock at 0 whenever the load is still streaming OR
            // present-but-not-render-ready, so `tick` counts a CONTINUOUS render-ready dwell, not just
            // frames since the cover cleared.
            // WAIT-FOR-INPUT-TO-REGISTER (user 2026-07-18): advance to the next switch only once this
            // load's INPUT has REGISTERED -- i.e. the can-move probe proved the char MOVES under injected
            // input for THIS fresh-deser epoch (CAN_MOVE_CONFIRMED + MOVE_PROBE_EPOCH, the reliable
            // signal; render_ready is a known false-negative oracle). Also require native MoveMap/requestCode
            // settlement, because the load2 bug can move while still stuck at requestCode=1/mms18 with draw
            // disabled. Do NOT blow through on a timer while current-epoch input never took. A load that
            // never proves movement within the freeze deadline is the FROZEN case -> per the strict parity,
            // drive the next load to recover it. (render_ready/load_done kept only for the diagnostic log
            // line.)
            let committed = deser >= expected_deser && player_up;
            let (request_code, mms_live, _native_load_settled) = sq_repro_native_load_state();
            // RELIABLE reload-movable gate (user 2026-07-21: stop waiting on WRONG semaphores). The move
            // proof (CAN_MOVE_CONFIRMED) is foreground-limited and never fires here, and the mms
            // native_load_settled is stale-owner unreliable -- with no deadline fallback, together they hung
            // load3 FOREVER. Advance on the vanilla movable SIGNATURE instead: reload committed (fresh-deser
            // reached + player present) + render-group ready + the world clock LIVE for THIS epoch
            // (BOOT_VIEW_EPOCH_WORLD_LIVE == deser, which already carries a >=1s world-clock settle from the
            // play_time_live oracle). Game-global, valid on the native path, reliable across runs.
            let epoch_world_live =
                crate::constants::BOOT_VIEW_EPOCH_WORLD_LIVE.load(Ordering::SeqCst) == deser;
            // Require the ACTUAL movement proof (verdict==1: the can-move probe confirmed genuine
            // injected-stick movement for THIS reload epoch) before arming the next switch, so each reload
            // PROVES movement -- not just render-ready (render-group fires before the char is controllable,
            // so the previous gate armed switch #2 while load2 was still finalizing). A frozen reload must
            // remain a failed epoch until the bounded run tears down; force-switching used to overwrite the
            // portrait target mid-window and made the required per-epoch readiness proof impossible.
            let move_verdict_proven =
                crate::constants::HARNESS_MOVE_VERDICT.load(Ordering::SeqCst) == 1;
            let move_proven = committed && render_ready && epoch_world_live && move_verdict_proven;
            if move_proven {
                let completed = switch_index + 1;
                SQ_REPRO_SWITCH_INDEX.store(completed, Ordering::SeqCst);
                if completed >= sq_repro_target_switches() {
                    append_autoload_debug(format_args!(
                        "sq-repro: switch #{completed}/{} reload MOVABLE+SETTLED (fresh_deser={deser}) -- all target switches done -> DONE",
                        sq_repro_target_switches(),
                    ));
                    sq_repro_transition(SQ_REPRO_STATE_DONE);
                    return;
                }
                // More loads to drive: arm the NEXT switch the SAME menu-free programmatic way (no menu-nav,
                // no focus-steal) instead of OPEN_MENU. bd CORRECT-disable-custom-onclick.
                sq_repro_begin_switch();
                if let Ok(base) = game_rva(0) {
                    let slot = sq_repro_target_slot();
                    unsafe { crate::experiments::switch_slot_arm_programmatic(base, slot) };
                    append_autoload_debug(format_args!(
                        "sq-repro: switch #{completed}/{} reload MOVABLE (fresh_deser={deser} requestCode={request_code}/mms={mms_live}) -> PROGRAMMATIC arm switch #{}/{} target_slot={slot}; WAIT_RELOAD",
                        sq_repro_target_switches(),
                        completed + 1,
                        sq_repro_target_switches(),
                    ));
                }
                sq_repro_transition(SQ_REPRO_STATE_WAIT_RELOAD);
                return;
            }
            SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
            let waited = SQ_REPRO_WAIT_RELOAD_FRAMES.fetch_add(1, Ordering::SeqCst);
            if waited.is_multiple_of(SQ_REPRO_WAIT_RELOAD_LOG_EVERY) {
                append_autoload_debug(format_args!(
                    "sq-repro: WAIT_RELOAD gates (switch #{}/{} waited_frames={waited}): fresh_deser={deser}/{expected_deser} player_up={player_up} can_move={} move_epoch={} render_ready={render_ready} load_done={load_done} fake_cover={fake_cover} requestCode={request_code} mms={mms_live}",
                    switch_index + 1,
                    sq_repro_target_switches(),
                    CAN_MOVE_CONFIRMED.load(Ordering::SeqCst),
                    MOVE_PROBE_EPOCH.load(Ordering::SeqCst)
                ));
            }
            // A frozen epoch is evidence, not permission to overwrite it with the next target. Log the
            // deadline once and keep the state stable until movement proves or the run's global cap tears
            // down. This makes every requested switch independently accountable.
            if committed && !move_proven && waited == SQ_REPRO_FREEZE_RECOVERY_DEADLINE {
                append_autoload_debug(format_args!(
                    "sq-repro: switch #{}/{} FROZEN at deadline {SQ_REPRO_FREEZE_RECOVERY_DEADLINE}f (fresh_deser={deser} load_done={load_done} fake_cover={fake_cover}) -- NOT arming the next switch; preserving this failed epoch for the oracle",
                    switch_index + 1,
                    sq_repro_target_switches(),
                ));
            }
        }
        _ => {
            set_pad(0);
        }
    }
}

pub(crate) unsafe extern "system" fn system_quit_profile_load_confirmed_hook(
    action_obj: usize,
) -> usize {
    let orig = SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileLoadDialog confirmed-load trampoline unset for action=0x{action_obj:x} -- fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    let dialog =
        unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let profile_window = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let system_quit_profile_active = dialog != TITLE_OWNER_SCAN_START_ADDRESS
        && profile_window != 0
        && dialog == profile_window
        && SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    if !system_quit_profile_active {
        return unsafe { original(action_obj) };
    }

    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) >= SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        && SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT.load(Ordering::SeqCst) != 0
    {
        // Close ProfileSelect via the NATIVE cancel/back close (FUN_1407ac980: SetResult(Failed) +
        // window close vmethod) instead of arming the confirm-LOAD. Arming the load (writing
        // load_job_ctx+0x14c=2, the coupled Success-close path) makes the game enter an IN-WORLD
        // load/warp transition (GameMan.saveState/b80 -> 2 -> DoSaveStuff). Even with the actual
        // deserialize skipped by the FUN_14067b290 guard, that half-started transition sticks the game
        // at a loading screen and BLOCKS the return-title chain from ever running (observed 2026-07-01:
        // stuck, return_title functor_call_count=0, save_state=3, player still present). The cancel-close
        // pops the ProfileSelect window WITHOUT starting any load, so the menu-pump return-title chain
        // tears the world down cleanly and the autoload loads the picked slot at a clean title. This
        // runs in menu-pump ownership (this IS the native confirm callback) and one-shot -- not the racy
        // game-task tick. See bd system-quit-load-profile-6runs-state-2026-07-01.
        let load_job_ctx = unsafe { safe_read_usize(dialog + 0x1cc8) }.unwrap_or(0);
        if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
            match game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) {
                Ok(close_addr) => {
                    let close_fn: unsafe extern "system" fn(usize) =
                        unsafe { std::mem::transmute(close_addr) };
                    unsafe { close_fn(dialog) };
                    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.store(1, Ordering::SeqCst);
                    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => append_autoload_debug(format_args!(
                    "system-quit-dup: confirm cancel-close ABORT -- failed to resolve close rva 0x{SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA:x}"
                )),
            }
        }
        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileSelect confirm CANCEL-CLOSED action=0x{action_obj:x} dialog=0x{dialog:x} load_job_ctx=0x{load_job_ctx:x}; NO load-mode armed -> no in-world load transition -> return-title tears down + autoload loads at clean title"
        ));
        return 0;
    }

    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect confirmed-load transition ALLOWED action=0x{action_obj:x} dialog=0x{dialog:x}; actual load/deser is guarded at LoadJobContext::Run"
    ));
    unsafe { original(action_obj) }
}

/// PORTRAIT RETARGET + boot-view cover rearm for an own-menu character switch -- SHARED between the
/// USER ProfileSelect arm (`system_quit_arm_quickload_autoload`) and the deterministic control-file
/// arm (`switch_slot_arm_programmatic`), the same no-drift rule as `reset_switch_reload_latches`
/// (bd er-effects-rs-dpf6: the programmatic arm previously skipped the whole portrait/cover
/// lifecycle, so agent probes could not exercise the user path's window reset / FPS bail / publish
/// race at all). Game thread only.
///
/// PORTRAIT RETARGET (user 2026-07-03): the user just confirmed a NEW character for load, so the
/// loading-screen portrait should render THAT character, not the one still resident (ac0). Make it
/// before-break: retarget the spare/render to the selected slot (portrait_target_slot now returns
/// it) and RE-ENGAGE the drive (clear the per-window freeze) so the new model renders + gets its
/// depth mask -- but do NOT touch LOADING_BG_PORTRAIT_RGBA / PROFILE_HAVE_KEYED_FRAME, so the prior
/// masked head keeps displaying until the new model's first KEYED frame replaces it (no opaque
/// flash, no blank). Clear the stale spare candidate (captured for the old character before this
/// confirm) so the teardown-spare re-targets the new slot, and drop the depth-mask cache so the new
/// silhouette is computed fresh rather than bridged from the old head.
pub(crate) unsafe fn portrait_retarget_and_rearm_for_switch(selected_slot: i32, source: &str) {
    PROFILE_SPARE_CANDIDATE.store(0, Ordering::SeqCst);
    PROFILE_SPARE_CANDIDATE_MODEL.store(0, Ordering::SeqCst);
    PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
    invalidate_portrait_depth_mask();
    // ORPHAN RECLAIM AT SWITCH ARM (second-load foreign-head fix, pixel-proven 2026-07-06 run
    // jsm-slotstats2-switchqa). The prior window's spared renderer parks in PROFILE_SPARE_ORPHAN at the
    // load-complete reset and was only delete-enqueued inside profile_renderer_teardown_spare_hook --
    // but the System-Quit switch path never fires that native teardown-all (spare_hits stayed 1,
    // orphans_deleted 0 across the whole run), so the orphan lived through the NEXT loading window with
    // its model + offscreen scene still registered, rendering the PREVIOUS character's head every frame.
    // The new window's readback then published that head under the correctly-kicked new renderer
    // (window-2 RT dump structure-correlated 0.92 with the window-1 character). Reclaim it HERE, on the
    // game thread at the confirm press (same delay-delete path as the spare hook), so the new window's
    // offscreen render belongs to the new character alone.
    let orphan = PROFILE_SPARE_ORPHAN.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        let deleted = unsafe { delay_delete_enqueue_renderer(orphan) };
        ownership_release(OwnedClass::SparedRenderer);
        append_autoload_debug(format_args!(
            "loading-portrait: reclaimed prior spared renderer 0x{orphan:x} at switch confirm via CSDelayDeleteMan enqueued={deleted} (second-load foreign-head fix)"
        ));
    }
    PROFILE_PORTRAIT_RETARGETS.fetch_add(1, Ordering::SeqCst);
    // OFFSCREEN-SIZE ROW FIRST (2026-07-30, different-slot 256x256 root cause): the selected slot is
    // known HERE, before any of this switch's profile-table builds (ours at the loading screen AND the
    // native TitleTopDialog rebuild at the title transition). Patch its offscreen-size row now so every
    // renderer constructed for this switch snapshots the full-size RT; waiting for the loaded-slot flip
    // left the row native 128 until after both builds (run 20260730-202840).
    {
        let base = game_rva(0).unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if base != 0 && base != TITLE_OWNER_SCAN_START_ADDRESS {
            let patched = unsafe { patch_profile_offscreen_size_for_slot(base, selected_slot) };
            if !patched {
                append_autoload_debug(format_args!(
                    "loading-portrait: offscreen-size row patch DID NOT land for selected slot {selected_slot} at retarget -- switch-window portrait will build native-size (256) until a later patch succeeds"
                ));
            }
        }
    }
    // CONFIRM TIMESTAMP (bd er-effects-rs-dpf6 Phase 1): the first portrait publish after this confirm
    // consumes it to measure oracle_portrait_confirm_to_publish_ms (the publish-race latency).
    PORTRAIT_CONFIRM_MS.store(
        crate::experiments::boot_view_epoch_ms().max(1) as usize,
        Ordering::SeqCst,
    );
    append_autoload_debug(format_args!(
        "loading-portrait: RETARGET to selected slot {selected_slot} at confirm (make-before-break: drive re-engaged, prior masked head holds until the new keyed frame; source={source})"
    ));
    rearm_boot_progress_for_own_menu_load(selected_slot, source);
}

pub(crate) unsafe fn system_quit_arm_quickload_autoload(selected_slot: i32, source: &str) {
    const NO_SLOT: usize = usize::MAX;
    if selected_slot < 0 {
        append_autoload_debug(format_args!(
            "system-quit-quickload: not arming autoload from {source} -- invalid selected_slot={selected_slot}"
        ));
        return;
    }
    let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
    if system_dialog == 0 || system_dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(NO_SLOT, Ordering::SeqCst);
        SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-quickload: not arming direct native chain from {source} -- missing preserved original System dialog selected_slot={selected_slot}"
        ));
        return;
    }
    // DISABLED (2026-07-01): the CSGaitemImp deserialize/lookup/finalize guards only ever CORRUPT
    // the gaitem singleton -- emptying gaitemInsTable handles left a garbage non-canonical entry that
    // crashed GetGaitemIns->GetGaitemHandle (live 0x6710c0). They were a doomed attempt to make the
    // in-world load "safe"; we now BLOCK the in-world load-job entirely (see the robust gate in
    // system_quit_profile_load_job_run_hook) and return to title + autoload instead, so no in-world
    // gaitem deserialize should run. Leaving them installed would additionally corrupt the AUTOLOAD's
    // own post-title load whenever it deserializes while phase is still 1..3. Not installing them lets
    // every real deserialize run natively. (Install fns retained for reference / bisecting.)
    // Install the load-ONLY guard so the picked slot is not deserialized into the still-live world
    // when the native confirm arms the load; it forwards the real load at a clean title (autoload).
    install_system_quit_inworld_load_guard();
    // Install the in-world load REQUEST guard: neutralizes the native RequestLoadSlot (FUN_14067b2f0)
    // so GameMan.saveState/b80 never reaches 2 during the switch. This is the TRUE source of the
    // NowLoading transition that froze the menu pump; blocking it here (not reactively) lets the
    // menu-pump-owned return-title chain run + tear the world down. Forwarded at a clean title.
    install_system_quit_request_load_slot_guard();
    // REVERTED 2026-07-16: wiring install_system_quit_gaitem_deserialize_hook() here (to backstop the
    // 0x67141a stale-table AV) was WORSE -- its handler's SKIP path during the return-title transition
    // (phase CONFIRMED..HANDOFF) leaves the gaitem singleton inconsistent and the game DL_PANICs from inside
    // the gaitem code (crash stk showed the panic called from game+0x671843). That broke the FIRST switch
    // (DL_PANIC before the load) whereas without the hook the first switch completes via continue_confirm.
    // The 0x67141a crash on the 2nd+ consecutive switch remains a separate, pre-existing issue to solve
    // without this SKIP-based hook (2026-07-01 already noted these gaitem guards corrupt the singleton).
    // Re-arm the continue_confirm guard's one-shot: the upcoming clean-title confirm must drive a
    // fresh deserialize of THIS switch's picked slot before it streams (the hook itself is installed
    // unconditionally at attach; see install_system_quit_continue_confirm_hook).
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(0, Ordering::SeqCst);
    // Re-arm the menu-free clean-title switch reload one-shot so every switch (not just the first) can
    // drive its own picked-slot feed-deserialize -> continue_confirm (own_load_switch_reload_fire).
    SYSTEM_QUIT_SWITCH_MENU_FREE_RELOAD_FIRED.store(0, Ordering::SeqCst);
    // Reset the switch-reload FD4-IO phase + Phase-3 outgoing-teardown latches. The USER ProfileSelect arm
    // was MISSING these (only switch_slot_arm_programmatic had them), so a user-driven CONSECUTIVE load
    // inherited SWITCH_RELOAD_FD4IO_COMMITTED=1 stale from the prior load -> own_load_switch_reload_fire
    // short-circuited at the already-committed guard -> no SUBMIT -> FRESH_DESER_DONE stuck 0 -> the b78
    // guard wrote GameMan requestedSaveSlotLoad=-1 every frame -> native pump gate false -> world torn down
    // at ENTERING WORLD = the load3 softlock (bd compounding-reload-two-roots-...-chainB-stale-fd4io-latch-b78-2026-07-23).
    // These are the FD4IO/OUTGOING latches that switch_slot_arm_programmatic already resets safely -- NOT
    // the RETURN_TITLE/FINAL_FUNCTOR counters that the 2026-07-02 bisect below found regressive.
    crate::experiments::own_load::reset_switch_reload_latches();
    // Re-arm the return-title one-shots so EVERY switch (not just the first) tears the world down.
    // Both are consumed by the first switch and never reset otherwise, so a second switch in the same
    // session would skip the native return-title REQUEST (`== 0` gate, sets saveRequested+bc4=1) and
    // the final-functor submit (compare_exchange 0->1 gate), leaving the second switch stuck in-world.
    // Resetting them here (the per-switch arm point) is the durable fix for repeatable switching
    // (er-effects-rs-qwj). SUBMIT_COUNT is intentionally NOT reset: title.rs uses it as a `> 0` enable
    // and it re-increments before the final functor needs it.
    // BISECT 2026-07-02: these two resets regressed even the SINGLE-switch reload (base f59b2af
    // passes, adding them causes a SECOND title bounce after the load / new-game flash). Disabled
    // while isolating; a switch-#2-safe re-arm will be reinstated once the mechanism is understood.
    // SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.store(0, Ordering::SeqCst);
    // SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(selected_slot as usize, Ordering::SeqCst);
    // SPURIOUS-vs-GENUINE arm discriminator (2026-07-18). Record whether the local player is ABSENT at
    // the instant of the arm. A spurious boot self-reload arms from the title/menu (player absent) while a
    // genuine in-world switch arms with the player present. The in-world time-based disarm keys on this so
    // it only cancels the spurious boot self-reload, never a real switch. See profile_render.rs
    // SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT and bd repeatable-multi-save-consolidated-plan-2026-07-18.
    let arm_player_absent = unsafe { PlayerIns::local_player_mut() }.is_err();
    SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT.store(usize::from(arm_player_absent), Ordering::SeqCst);
    unsafe { portrait_retarget_and_rearm_for_switch(selected_slot, source) };
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED, Ordering::SeqCst);
    OWN_STEPPER_SLOT.store(selected_slot, Ordering::SeqCst);
    PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU, Ordering::SeqCst);
    TFC_CONTINUE_FIRED.store(0, Ordering::SeqCst);
    TFC_FORCED_CONTINUE_HANDOFF_MS.store(0, Ordering::SeqCst);
    OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.store(0, Ordering::SeqCst);
    TFC_LOAD_VEC_WAIT_TICKS.store(0, Ordering::SeqCst);
    OWN_STEPPER_MENU_OPENED.store(OWN_STEPPER_MENU_OPENED_NO, Ordering::SeqCst);
    TITLE_ACCEPT_BYTE_GATE_FIRED.store(false, Ordering::SeqCst);
    TITLE_OWNER_PTR.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    TITLE_OWNER_SCAN_COUNTDOWN.store(TITLE_OWNER_SCAN_COUNTDOWN_READY, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(false, Ordering::SeqCst);
    // Re-arm the product-core-autoload CONTINUE DRIVER for repeatable switching (2026-07-15): FULLREAD_PHASE is
    // a one-shot that reaches FULLREAD_PHASE_DONE after the first switch's Continue and then early-returns
    // (product_continue.rs:282), so the 2ND consecutive switch's return-title reaches the title but nothing
    // drives its native Continue -> stuck at the covered title (the "black screen"). Reset the phase to SUBMIT
    // and drop the stale MENU_CONTINUE_* row/router pointers captured for the previous switch's (torn-down)
    // menu, so the driver re-captures + re-submits the Continue for THIS switch. Idempotent one-shots reset to
    // their init values, exactly like the per-switch latches above; the continue_confirm/world-up guards still
    // prevent driving a load into a live world. (Distinct from the return-title one-shots at 868-869, which the
    // 2026-07-02 bisect showed regress the single switch -- those stay disabled.)
    FULLREAD_PHASE.store(FULLREAD_PHASE_SUBMIT, Ordering::SeqCst);
    FULLREAD_DRAIN_WAITS.store(0, Ordering::SeqCst);
    MENU_CONTINUE_ITEM.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_ENTRY.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_FUNCTOR.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_DOCALL.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_ROUTER.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_INDEX.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_POST_RETURN_TITLE_FIRED.store(0, Ordering::SeqCst);
    PROFILE_REFRESH_KICKED.store(0, Ordering::SeqCst);
    PORTRAIT_RENDER_WINDOW_DONE.store(0, Ordering::SeqCst);
    // Reset the switch-outcome oracle so THIS switch's classification starts fresh (see the atomics' doc).
    SWITCH_ORACLE_TICK.store(0, Ordering::SeqCst);
    SWITCH_ORACLE_STABLE_FRAMES.store(0, Ordering::SeqCst);
    SWITCH_ORACLE_MAX_STABLE_FRAMES.store(0, Ordering::SeqCst);
    // SWITCH-2 SOFT-LOCK FIX (arm-point pre-clear, 2026-07-16). RE of the 1.16.1 dump proved the switch-2
    // freeze is the native quit-save (`ShouldSave` 0x1406794c0) aborting on a stale
    // `CSMenuMan->disableSaveMenu` (+0x13c, read by `CanShowSaveMenu` 0x14080d150) left set from the prior
    // switch's menu flow -- so `bc4` freezes at 1 and the world never tears down. Clear it HERE, the moment
    // this switch arms its return-title, so the gate is already open before the quit-save orchestrator runs
    // (belt-and-suspenders with the per-frame game-task clear in product_core_autoload_tick and the menu-pump
    // clear in system_quit_restore_real_system_windows). No-op / inert on switch 1 (its byte is already 0);
    // reuses the shared startup_hooks helper. SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT is the runtime
    // semaphore: >0 on a switch == that switch's quit-save was gated OFF and we unblocked it.
    let base = game_rva(0).unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if base != TITLE_OWNER_SCAN_START_ADDRESS {
        let dsm_prev =
            unsafe { system_quit_clear_disable_save_menu(base, "arm-quickload-return-title") };
        append_autoload_debug(format_args!(
            "system-quit-quickload: arm pre-clear CSMenuMan->disableSaveMenu was {dsm_prev} (>0 = switch-2 quit-save was BLOCKED; cleared so bc4 can pump 1->2->3 and the world tears down) selected_slot={selected_slot} source={source}"
        ));
    }
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(
        SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
        Ordering::SeqCst,
    );
    append_autoload_debug(format_args!(
        "system-quit-quickload: armed product Continue autoload selected_slot={selected_slot} source={source}; will direct-submit native return-title chain once ProfileSelect closes system_dialog=0x{system_dialog:x}"
    ));
}

/// Guard on the load-only routine `FUN_14067b380(slot)`. While the in-world System->Quit->Load-Profile
/// transition is active (phase in CONFIRMED..AUTOLOAD_HANDOFF) AND the old world is still up (local
/// player present), skip the deserialize+warp and report success -- so `DoSaveStuff` completes (clears
/// its pending slot) and ProfileSelect closes, but nothing loads into the live world. At a clean title
/// (player absent, or phase past the transition) it forwards to the real load so the autoload works.
pub(crate) unsafe extern "system" fn system_quit_inworld_load_skip_hook(slot: i32) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let in_transition = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
    if in_transition && world_up {
        let n = SYSTEM_QUIT_INWORLD_LOAD_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-quickload: in-world load SKIPPED #{n} slot={slot} phase={phase} (old world still up) -- ProfileSelect close proceeds; return-title tears down; autoload loads at clean title"
        ));
        // FUN_14067b380 returns 1 on success; report success without deserializing so DoSaveStuff's
        // caller advances (it then clears MoveMapStep+0x12c) instead of retrying the in-world load.
        return 1;
    }
    SYSTEM_QUIT_INWORLD_LOAD_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_INWORLD_LOAD_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(i32) -> usize = unsafe { std::mem::transmute(orig) };
    let ret = unsafe { original(slot) };
    let selected = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if ret != 0 && selected < TITLE_PROFILE_SLOT_COUNT && slot == selected as i32 {
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(1, Ordering::SeqCst);
        // Which slot's deserialize completed (slot+1) -- ground truth for the published-vs-loaded
        // portrait oracle. See er_telemetry counters.
        er_telemetry::counters::SYSTEM_QUIT_FRESH_DESER_DONE_SLOT
            .store((slot + 1) as usize, Ordering::SeqCst);
        SYSTEM_QUIT_QUICKLOAD_PHASE.store(SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE, Ordering::SeqCst);
        let gm = game_man_ptr_or_null();
        if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *mut i32) = OWN_STEPPER_SLOT_NONE;
            }
        }
        if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
            er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
        }
        let n = SYSTEM_QUIT_INWORLD_LOAD_ALLOW_COUNT.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-quickload: native slot deserialize proof OK via 0x67b290 slot={slot} ret={ret} prior_phase={phase} world_up={world_up} allow_count={n} -> phase IDLE, cleared GameMan+0xb78/save_requested; native owns warp_requested finalize/autoclear"
        ));
    }
    ret
}

pub(crate) fn install_system_quit_inworld_load_guard() {
    if SYSTEM_QUIT_INWORLD_LOAD_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for in-world load guard failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_INWORLD_LOAD_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve in-world load rva 0x{SYSTEM_QUIT_INWORLD_LOAD_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_inworld_load_skip_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_INWORLD_LOAD_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable in-world load guard failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    SYSTEM_QUIT_INWORLD_LOAD_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked in-world load routine 0x{addr:x}; picked-slot deserialize skipped while old world up, forwarded at clean title"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued in-world load guard failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new in-world load guard failed: {status:?}"
        )),
    }
}

/// Guard on the native in-world load REQUEST `CS::GameMan::RequestLoadSlot(slot)` (FUN_14067b2f0, live
/// 0x67b200). This is the TRUE source of GameMan.saveState/b80=2 for an explicit-slot in-world load:
/// the per-frame MoveMapStep load steps call it once the confirmed ProfileSelect chain pushes the map
/// machine into loading, and it sets saveState=2, which starts the 02_904_NowLoading transition that
/// freezes the menu pump so the queued return-title chain can never run. During the in-world
/// System->Quit->Load-Profile transition (phase active AND old world still up / local player present)
/// we return "not armed" (0) WITHOUT calling the original, so saveState never reaches 2: no NowLoading,
/// the pump keeps running, and the menu-pump-owned return-title chain tears the world down. Once the
/// world is gone (player absent) or the switch is idle, we forward to the real request -- so the
/// clean-title autoload and any normal load work. The boot/Continue autoload uses the distinct sentinel
/// variants (FUN_14067b290 slot 10 / FUN_14067b570 slot 0xb), which this hook does not touch. See bd
/// system-quit-loadjob-success-commits-phantom-load-2026-07-01.
pub(crate) unsafe extern "system" fn system_quit_request_load_slot_hook(slot: u32) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    // Range-gate like the sibling system_quit_inworld_load_skip_hook (NOT `!= IDLE`): the clean-title
    // reload runs at AUTOLOAD_HANDOFF and re-creates a present player, so a `!= IDLE` gate would
    // neutralize the RELOAD's own RequestLoadSlot mid-load. Neutralize only during the first-world
    // transition [CONFIRMED, AUTOLOAD_HANDOFF); forward natively at AUTOLOAD_HANDOFF so the reload loads.
    let switch_active = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
    if switch_active && world_up {
        let n = SYSTEM_QUIT_REQUEST_LOAD_SLOT_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 8 || n.is_multiple_of(120) {
            append_autoload_debug(format_args!(
                "system-quit-quickload: in-world load REQUEST neutralized #{n} slot={slot} phase={phase} (old world still up) -- saveState/b80 kept idle so no NowLoading; return-title tears down + autoload loads at clean title"
            ));
        }
        // RequestLoadSlot returns 0 when it declines to arm (saveState!=0 or profile check fails). We
        // return the same "not armed" result so the caller MoveMapStep treats it as no-load-yet instead
        // of entering the in-world load transition.
        return 0;
    }
    SYSTEM_QUIT_REQUEST_LOAD_SLOT_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_REQUEST_LOAD_SLOT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(u32) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { original(slot) }
}

pub(crate) fn install_system_quit_request_load_slot_guard() {
    if SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for RequestLoadSlot guard failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_REQUEST_LOAD_SLOT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve RequestLoadSlot rva 0x{SYSTEM_QUIT_REQUEST_LOAD_SLOT_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_request_load_slot_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_REQUEST_LOAD_SLOT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable RequestLoadSlot guard failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked in-world load request RequestLoadSlot 0x{addr:x}; saveState/b80=2 arm neutralized while old world up, forwarded at clean title"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued RequestLoadSlot guard failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new RequestLoadSlot guard failed: {status:?}"
        )),
    }
}

/// Guard on the native title Continue confirm `0x140b0e180` (rcx = the {[+8]=owner} shim; reads
/// GameMan+0xc30 -> owner+0xbc -> SetState(5); picks NO slot). Static RE 2026-07-02 proved the
/// post-switch clean-title reload streams the PRE-SWITCH GameMan/PlayerGameData state: no fresh
/// deserialize of the picked slot runs anywhere on that path, so the resident (original) character
/// gets re-streamed -- the wrong-character bug. While a System->Quit->Load-Profile switch is active
/// this hook drives ONE fresh synchronous feed-deserialize of the PICKED slot
/// (`own_load_feed_deserialize`: on-disk read -> gated 0x67b100 feed -> native parser 0x67b290)
/// BEFORE forwarding, so ac0/c30/PGD all become the picked slot and the confirm streams the right
/// character. Fail-closed: if the fresh deserialize cannot be proven, the confirm is BLOCKED --
/// streaming stale state would load the wrong character and the post-load autosave would then write
/// it back into the picked slot. Boot autoloads and normal play (phase IDLE) pass through
/// untouched. See bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02.
pub(crate) unsafe extern "system" fn system_quit_continue_confirm_hook(
    shim: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // RENDER-HANDOFF FIX ARM (bd er-effects-rs-um9g): this Continue/Load confirm is the common trigger
    // for BOTH the boot autoload and the in-world reload; it captures GameMan+0xc30 into the TitleStep,
    // then forwards to SetState5 -> STEP_PlayGame -> InGameStep::RequestMoveMap. On our redirect load the
    // captured BlockId can be stale/-1, which makes RequestMoveMap skip building the world-res loadlist
    // path and stall at WorldResWait. Arm the RequestMoveMap fixup here so the upcoming RequestMoveMap
    // substitutes the freshly-deserialized saved-map BlockId (armed-only + invalid-param2-only, so it is
    // a no-op for a load whose BlockId is already valid).
    crate::experiments::own_load::arm_request_move_map_fixup();
    // Continue-trace compat: this unconditional hook replaced the trace-set `cap_continue_confirm`
    // hook on the same address (two MinHooks on one target fail -- the install_c30_writer_hook
    // precedent), so reproduce its logging + confirm latch exactly when tracing is on.
    if trace_continue_enabled() && !continue_trace_disabled() {
        let owner = if shim != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                safe_read_usize(shim + OWN_STEPPER_SHIM_OWNER_IDX * core::mem::size_of::<usize>())
            }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        append_continue_trace(format_args!(
            "CAP continue_confirm this=0x{shim:x} owner=0x{owner:x} {} {}",
            trace_callers_summary(),
            b80_mount_trace_summary()
        ));
        OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    // Inclusive of AUTOLOAD_HANDOFF (unlike the in-world guards' half-open range): the clean-title
    // reload's confirm fires at TITLE_OWNER_SEEN or AUTOLOAD_HANDOFF and the fresh deserialize is
    // exactly what phase 4 needs; the one-shot DONE latch prevents repeats after success.
    let switch_active = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..=SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    if switch_active {
        let selected = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
        let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
        if world_up {
            // A title-flow confirm while the old world is still up is not a state we ever drive;
            // never deserialize into a live world (that is the crash the whole switch avoids).
            // Forward and log loudly -- the in-world load guards protect the load paths.
            // Counted, not just logged: this forward reaches the unconditional ALLOW increment
            // below, so without its own bucket it would break the load-count decomposition
            // (`allow == fresh_deser + non_switch + world_up`) with no way to tell which bucket lost
            // it. See er_telemetry::load_count.
            SYSTEM_QUIT_CONTINUE_CONFIRM_WORLD_UP_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm called while OLD WORLD STILL UP phase={phase} selected={selected} shim=0x{shim:x} -- forwarding WITHOUT fresh deserialize (unexpected caller)"
            ));
        } else if SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0
            && selected >= TITLE_PROFILE_SLOT_COUNT
        {
            let n = SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm BLOCKED #{n} -- switch active (phase={phase}) but no valid picked slot ({selected}); refusing to stream stale pre-switch state"
            ));
            return 0;
        } else {
            let slot = selected as i32;
            let base = game_rva(0).unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let _gm = game_man_ptr_or_null();
            let native_slot_proven =
                SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 1;
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm intercepted at clean title phase={phase} slot={slot} native_slot_proven={native_slot_proven} shim=0x{shim:x}"
            ));
            if !native_slot_proven {
                // Do NOT disarm GameMan+0xb78 here. The 2026-07-30 slot-1 repro hit this unproven
                // branch, cleared the requested slot to -1, then waited forever for stable-world proof
                // that the native stream could no longer reach. The same b78-as-warp-target rule below
                // applies even when the earlier proof latch missed.
                // The `#n` label comes from this branch's OWN counter. It used to come from
                // ALLOW_COUNT, which also increments unconditionally at the tail of this hook -- so
                // one call incremented the total-load witness TWICE and inflated it by one per
                // unproven reload. Both captured runs had native_slot_proven=true throughout, which
                // is the only reason their allow counts were exact.
                let n = SYSTEM_QUIT_CONTINUE_CONFIRM_UNPROVEN_FORWARD_COUNT
                    .fetch_add(1, Ordering::SeqCst)
                    + 1;
                SYSTEM_QUIT_QUICKLOAD_PHASE.store(
                    SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF,
                    Ordering::SeqCst,
                );
                append_autoload_debug(format_args!(
                    "system-quit-quickload: continue_confirm FORWARD #{n} -- native requested-slot proof did not fire for slot={slot}; leaving GameMan+0xb78 armed as the warp target and holding phase at AUTOLOAD_HANDOFF until stable-world proof fires (runtime evidence: clearing b78 here strands the native stream; setting DONE/IDLE here lets the next switch overlap unfinished MoveMap)"
                ));
            }
            {
                let n = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT
                    .fetch_add(1, Ordering::SeqCst)
                    + 1;
                // LOAD COMMITTED -> get out of the way. The forwarded continue_confirm below fires
                // SetState5, which streams the picked character. Return the switch machine to IDLE so
                // the product-core autoload's switch branch STOPS (title.rs: it keeps arming
                // GameMan+0xb78 = an in-world MoveMapStep load of the slot, and keeps re-driving the
                // title, while phase >= RETURN_TITLE_REQUESTED). Left armed, that redundant b78 load
                // competes with this SetState5 stream, stalls the title owner at state 6, and bounces
                // the freshly-loaded world back to the title ~4s later (the post-load instability the
                // earlier single-switch milestone missed -- it tore down before the bounce). IDLE also
                // makes the in-world load guards inert (they gate on [CONFIRMED, AUTOLOAD_HANDOFF)), so
                // the native world stream is unobstructed, and leaves the session clean for the next
                // switch (also the durable fix for the post-switch hygiene issue er-effects-rs-qwj).
                SYSTEM_QUIT_QUICKLOAD_PHASE.store(
                    SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF,
                    Ordering::SeqCst,
                );
                // Keep GameMan+0xb78 armed through SetState5/MoveMap finalize. Runtime evidence from
                // samechar-3x-phaseearly shows clearing b78 to -1 before the warp finalize leaves the
                // loaded target without a valid requested-slot/warp target and TitleStep falls back to
                // title. A later post-resident proof point must clear it, but not before the world stream
                // has survived.
                // CLEAR the return-title "rebuild the title" request flags the final functor set for this
                // switch's teardown (restored 2026-07-16 after the DEFER experiment failed: the stuck load
                // has menuData+0x5d==0 to BEGIN with -- the functor never set it, incomplete teardown -- so
                // deferring OUR clear was inert). They are LEVEL flags nothing resets; left set on a
                // resident world they re-request quit-to-title (bounce). Undo them at the clean-title
                // Continue as before.
                let menu_man = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
                    .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if menu_man != TITLE_OWNER_SCAN_START_ADDRESS
                    && unsafe { is_heap_aligned_ptr(menu_man) }
                    && let Some(menu_data) =
                        unsafe { safe_read_usize(menu_man + CS_MENU_MAN_MENU_DATA_OFFSET) }
                    && menu_data != TITLE_OWNER_SCAN_START_ADDRESS
                    && unsafe { is_heap_aligned_ptr(menu_data) }
                {
                    unsafe {
                        *((menu_data + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 0;
                        *((menu_data + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) as *mut u8) = 0;
                    }
                }
                unsafe { *((base + RETURN_TITLE_REBUILD_FLAG_DAT_RVA) as *mut u8) = 0 };
                // Clear GameMan.save_requested defensively (typed): the return-title REQUEST set it for
                // the teardown; a residual true would drive an immediate quit-save on the reload. Do NOT
                // clear GameMan.warp_requested here. Native full deserialize owns that flag; MoveMapStep
                // finalize case 8 consumes/autoclears it after advancing mms18.
                if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
                    er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
                }
                // REPEATABLE-SWITCH STATE RESTORE (er-effects-rs-qwj). The switch-#1 works but
                // switch-#2-stalls symptom is a pure precondition mismatch: these three return-title
                // one-shots are CONSUMED by this switch's teardown and gate the NEXT switch --
                // RETURN_TITLE_REQUEST_COUNT (native return-title REQUEST fires only when ==0,
                // startup_hooks 6922), DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT (menu-pump submit only
                // when ==0, 7162), FINAL_FUNCTOR_CALL_COUNT (final-functor compare_exchange 0->1,
                // title.rs 1690). Left set, switch #2 skips its return-title REQUEST + submit and
                // never tears the world down (observed: stuck at title state 10/10, bc4=0). Restoring
                // them to boot-fresh here makes every switch byte-identical to the first. This is the
                // SAFE edge (unlike the disabled arm-time reset above, which re-fires during teardown
                // and double-submits -> the single-switch bounce that regressed it): it runs once per
                // switch (fresh-deser latch), AFTER this switch's return-title machinery is fully
                // consumed. The phase remains AUTOLOAD_HANDOFF until the streamed load reaches a stable
                // world, so every return-title REQUEST/submit/final-functor gate must exclude
                // AUTOLOAD_HANDOFF; otherwise the reset counts can be consumed by a spurious second
                // return-title request that leaves bc4=3 stale and blocks the incoming MoveMap finalize.
                SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.store(0, Ordering::SeqCst);
                SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.store(0, Ordering::SeqCst);
                SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
                // Restore the per-switch MENU-WINDOW state to boot-fresh too -- the visual analogue of
                // the one-shots above (er-effects-rs-qwj). These trackers hold this switch's now-destroyed
                // IngameTop/OptionSetting/ProfileSelect windows; left stale, the NEXT switch's quit menu
                // (a) does not hide behind ProfileSelect (the hide keys off a valid tracked window, but
                // the stale pointer's vtable is zeroed on the torn-down window -> hid_top=false, so the
                // quit menu renders on top) and (b) its Quit Game / Return-to-Desktop rows act dead
                // because the menu is layered over a stale ProfileSelect. Resetting here -- the same
                // trackers the autopilot's sq_repro_begin_switch clears before each switch -- makes the
                // next quit-menu open repopulate them fresh via the MenuWindowJob::Run hook, so the hide
                // + input behave identically to the first switch. (Manual B-to-back had the same effect
                // by forcing a fresh window; this makes it automatic.)
                unsafe {
                    system_quit_reset_profile_select_state("post-switch-commit-menu-hygiene")
                };
                SYSTEM_QUIT_INGAME_TOP_WINDOW.store(0, Ordering::SeqCst);
                SYSTEM_QUIT_OPTION_SETTING_WINDOW.store(0, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: native Continue handoff commit OK #{n} slot={slot} -- forwarding continue_confirm so SetState5 streams; phase stays AUTOLOAD_HANDOFF until stable-world proof + keep GameMan+0xb78 armed through native finalize + cleared return-title rebuild flags (menuData+0x5d/0x5e, DAT, save_requested) + native-owned warp_requested finalize/autoclear + RESET return-title one-shots for the NEXT switch only (return-title gates exclude AUTOLOAD_HANDOFF)"
                ));
            }
        }
    }
    // TOTAL WORLD LOADS, boot included: exactly one increment per forwarded confirm. Every forward
    // is classified into exactly one bucket -- switch reload (FRESH_DESER_COUNT, == the load epoch),
    // old-world-up (WORLD_UP_COUNT), or neither, i.e. the boot/title Continue (NON_SWITCH_COUNT) --
    // so `allow == fresh_deser + non_switch + world_up` holds by construction and any drift is a
    // real telemetry fault. That identity is what `er_telemetry::load_count` audits, and the missing
    // boot bucket is exactly why a 3-load session reports oracle_current_load_epoch = 2.
    if !switch_active {
        SYSTEM_QUIT_CONTINUE_CONFIRM_NON_SWITCH_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(shim, b, c, d) }
}
