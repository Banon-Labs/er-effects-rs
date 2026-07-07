
/// Log `msg` exactly once for the current repro phase (`SQ_REPRO_STATE_TAPS` latches it), used when
/// a phase has issued all its edges and is now HOLDING until its transition is observed. Not a retry
/// budget -- a boolean latch so the "waiting" line is not spammed.
fn sq_repro_waiting_once(msg: &str) {
    if SQ_REPRO_STATE_TAPS.swap(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "sq-repro: {msg} (holding until observed; no re-tap)"
        ));
    }
}

/// Fail-closed bit outside the tab range. The current ProfileBack row fingerprint oracle reads the
/// native `PropertyEditDialog` backing table, not the rendered/menu-facing GFx row/list state the
/// user-visible bug corrupts. Until the visible row-content semaphore exists, a run that reaches
/// ProfileSelect->Back must be classified as harness-incomplete/failed, not pass.
const SQ_REPRO_PROFILE_BACK_VISIBLE_ORACLE_MISSING_MASK: usize =
    1usize << OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT;

/// Cumulative ProfileSelect OK-confirm count (cancel-close BLOCK + ALLOW). Legacy fallback signal:
/// the CONFIRM state's primary advance is the direct-arm phase observation; this count (an INCREASE
/// over the per-switch baseline, so switch #2 does not trip on switch #1's residual) only fires if
/// the pick fell through to the native confirm-box -> OK -> load-job chain.
fn sq_repro_confirm_count() -> usize {
    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.load(Ordering::SeqCst)
        + SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.load(Ordering::SeqCst)
}

/// The ProfileSelect slot the current switch loads (clamped to the target table).
fn sq_repro_target_slot() -> i32 {
    let i = SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst);
    SQ_REPRO_TARGET_SLOTS[i.min(SQ_REPRO_TARGET_SLOTS.len() - 1)]
}

/// How many back-to-back switches to drive in the legacy ProfileSelect harness path. The active Save
/// Game row validation path below is always-on and no longer reads an env selector.
fn sq_repro_target_switches() -> usize {
    SQ_REPRO_TARGET_SWITCHES.clamp(0, SQ_REPRO_TARGET_SLOTS.len())
}

fn sq_repro_option_tab_row_fingerprint(option_window: usize, tab: usize) -> Option<(usize, usize)> {
    const HEAP_LO: usize = 0x10000;
    if option_window < HEAP_LO || tab >= OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT {
        return None;
    }
    let composite = option_window + OPTIONSETTING_COMPOSITE_OFFSET;
    let dialog = unsafe {
        safe_read_usize(composite + OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET + tab * 8)
    }
    .unwrap_or(0);
    if dialog < HEAP_LO || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let count = unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }?;
    if count > 64 {
        return None;
    }
    let properties = dialog + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET;
    let aligned_properties = (properties + 0x7) & !0x7;
    let mut hash = 0xcbf29ce484222325usize;
    for byte in count.to_le_bytes() {
        hash ^= byte as usize;
        hash = hash.wrapping_mul(0x100000001b3usize);
    }
    for offset in 0..count.saturating_mul(EDIT_PROPERTY_SIZE) {
        let b = unsafe { safe_read_u8(aligned_properties + offset) }?;
        hash ^= b as usize;
        hash = hash.wrapping_mul(0x100000001b3usize);
    }
    Some((count, hash))
}

fn sq_repro_profile_back_record_baseline(option_window: usize, tab: usize) {
    let Some((count, hash)) = sq_repro_option_tab_row_fingerprint(option_window, tab) else {
        return;
    };
    SQ_REPRO_PROFILE_BACK_BASELINE_COUNTS[tab].store(count, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_BASELINE_HASHES[tab].store(hash, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_BASELINE_MASK.fetch_or(1usize << tab, Ordering::SeqCst);
}

fn sq_repro_profile_back_verify_tab(option_window: usize, tab: usize) {
    let Some((count, hash)) = sq_repro_option_tab_row_fingerprint(option_window, tab) else {
        return;
    };
    SQ_REPRO_PROFILE_BACK_VERIFY_COUNTS[tab].store(count, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_VERIFY_HASHES[tab].store(hash, Ordering::SeqCst);
    SQ_REPRO_PROFILE_BACK_VERIFY_MASK.fetch_or(1usize << tab, Ordering::SeqCst);
    let baseline_count = SQ_REPRO_PROFILE_BACK_BASELINE_COUNTS[tab].load(Ordering::SeqCst);
    let baseline_hash = SQ_REPRO_PROFILE_BACK_BASELINE_HASHES[tab].load(Ordering::SeqCst);
    if baseline_count != count || baseline_hash != hash {
        SQ_REPRO_PROFILE_BACK_MISMATCH_MASK.fetch_or(1usize << tab, Ordering::SeqCst);
    }
}

/// Legacy pause-at-menu mode is disabled while the always-on Save Game row repro is active.
fn sq_repro_pause_at_menu() -> bool {
    false
}

/// TAB-RETURN repro mode (agent-owned blank-pane harness): gated by GAME_DIR file
/// `er-effects-tab-return-repro.txt` or env `ER_EFFECTS_TAB_RETURN_REPRO=1`. When on, after the
/// OptionSetting opens the autopilot drives RIGHT to the last tab then LEFT back to Game Options and
/// dwells -- reproducing the blank the user reported (tab goes blank on return after the custom tab),
/// with NO Save Game / no load (save-safe). Takes precedence over the Save Game row path.
fn sq_repro_tab_return_mode() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TAB_RETURN_REPRO").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-tab-return-repro.txt")
        .exists()
}

/// Exact USER repro: System menu -> Quit tab -> Load Profile -> Back before selecting a profile ->
/// return to Game Options. This is the cross-populated-row bug path; it does not load a profile and
/// does not use the file picker. Gated separately so the older Save Game harness stays default.
fn sq_repro_profile_back_mode() -> bool {
    game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-profile-back-repro.txt")
        .exists()
}

/// SAVE-GAME ROW mode for the System->Quit repro autopilot. The main
/// `ER_EFFECTS_SYSTEM_QUIT_REPRO=1` gate still controls whether the repro harness runs at all.
fn sq_repro_save_game_only() -> bool {
    !sq_repro_tab_return_mode() && !sq_repro_profile_back_mode()
}

/// Enter a switch: capture the confirm-count baseline and clear the per-switch menu-window/cursor
/// signals so the state machine re-detects them fresh for this switch (they hold stale pointers from
/// the prior switch otherwise). Called before OPEN_MENU for every switch.
fn sq_repro_begin_switch() {
    SQ_REPRO_CONFIRM_BASELINE.store(sq_repro_confirm_count(), Ordering::SeqCst);
    SYSTEM_QUIT_INGAME_TOP_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_OPTION_SETTING_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.store(0, Ordering::SeqCst);
    SQ_REPRO_INITIAL_CURSOR.store(usize::MAX, Ordering::SeqCst);
    SQ_REPRO_WAIT_RELOAD_FRAMES.store(0, Ordering::SeqCst);
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

/// Fabricated gamepad wButtons for a phase that issues a FIXED list of button edges ONCE, in order,
/// then holds. `tick` is phase-local; each edge occupies one `INJECT_NAV_CYCLE` (the RE-grounded
/// edge hold+gap -- edge-triggered menu nav needs a multi-frame hold to register one step). Returns
/// `(wButtons_this_frame, holding)`: `holding` is true once every edge has been issued, so the
/// caller waits on an OBSERVED transition (never a timer or budget) to advance.
fn sq_repro_edges(tick: usize, edges: &[u16]) -> (u16, bool) {
    let edge_index = tick / INJECT_NAV_CYCLE;
    if edge_index >= edges.len() {
        return (0, true);
    }
    let asserted = (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN;
    (if asserted { edges[edge_index] } else { 0 }, false)
}

/// SELF-DRIVEN System->Quit->Load-Profile REPRO AUTOPILOT tick (gated by `system_quit_repro_enabled`).
/// Runs every game-task frame. The input block stays engaged in-world (see `block_input_enabled`) so
/// the fabricated gamepad is the ONLY input and no human press can contaminate the repro. Drives the
/// user's EXACT Xbox controller sequence by writing `SQ_REPRO_XINPUT_BUTTONS` (read by the XInput
/// poll hook -- the stage the game reads a gamepad from), advancing ONLY on observed menu-window /
/// cursor / activate transitions (never timers or tap budgets):
///   START -> IngameTop; UP,A -> OptionSetting; LB,DOWN,A -> ProfileSelect; one DOWN/UP off the
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
    // Driven entirely via the XInput poll hook; keep the DInput keyboard stamp clear every frame so
    // no stale key leaks while the block zeroes the real keyboard.
    crate::input_blocker::InputBlocker::get_instance().set_injected_key(DIK_NONE);
    let set_pad = |b: u16| SQ_REPRO_XINPUT_BUTTONS.store(b as usize, Ordering::SeqCst);
    let tick = SQ_REPRO_STATE_TICK.fetch_add(1, Ordering::SeqCst);

    match state {
        SQ_REPRO_STATE_WAIT_WORLD => {
            set_pad(0);
            let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
            if in_world && tick >= SQ_REPRO_WORLD_SETTLE_TICKS {
                sq_repro_begin_switch();
                if sq_repro_save_game_only() {
                    append_autoload_debug(format_args!(
                        "sq-repro: in-world settled ({SQ_REPRO_WORLD_SETTLE_TICKS} ticks) -> OPEN_MENU Save Game row mode; START (XInput 0x{XINPUT_GAMEPAD_START:04x}) to open the escape/system menu"
                    ));
                } else if sq_repro_pause_at_menu() {
                    append_autoload_debug(format_args!(
                        "sq-repro: in-world settled ({SQ_REPRO_WORLD_SETTLE_TICKS} ticks) -> OPEN_MENU PAUSE-AT-MENU mode (0 switches: stop at ProfileSelect, no load); START (XInput 0x{XINPUT_GAMEPAD_START:04x}) to open the escape/system menu"
                    ));
                } else {
                    append_autoload_debug(format_args!(
                        "sq-repro: in-world settled ({SQ_REPRO_WORLD_SETTLE_TICKS} ticks) -> OPEN_MENU switch #{}/{} target_slot={}; START (XInput 0x{XINPUT_GAMEPAD_START:04x}) to open the escape/system menu",
                        SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst) + 1,
                        sq_repro_target_switches(),
                        sq_repro_target_slot()
                    ));
                }
                sq_repro_transition(SQ_REPRO_STATE_OPEN_MENU);
            } else if !in_world {
                // Not in-world yet (boot autoload still loading): hold the settle counter at 0.
                SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
            }
        }
        SQ_REPRO_STATE_OPEN_MENU => {
            let ingame_top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
            if ingame_top != 0 {
                append_autoload_debug(format_args!(
                    "sq-repro: IngameTop opened window=0x{ingame_top:x} (escape/system menu) -> TO_SYSTEM (UP, A into the quit submenu)"
                ));
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_TO_SYSTEM);
                return;
            }
            let (btn, holding) = sq_repro_edges(tick, &[XINPUT_GAMEPAD_START]);
            if holding {
                sq_repro_waiting_once("OPEN_MENU: START issued, waiting for 02_000_IngameTop");
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_TO_SYSTEM => {
            let option_setting = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
            if option_setting != 0 {
                if sq_repro_tab_return_mode() {
                    append_autoload_debug(format_args!(
                        "sq-repro: OptionSetting opened window=0x{option_setting:x} -> TAB_RETURN (RB to the last/Quit tab, then LB back to Game Options, then dwell -- blank-pane repro)"
                    ));
                    set_pad(0);
                    SQ_REPRO_TAB_RETURN_PHASE.store(0, Ordering::SeqCst);
                    SQ_REPRO_TAB_RETURN_MAX_TAB.store(0, Ordering::SeqCst);
                    sq_repro_transition(SQ_REPRO_STATE_TAB_RETURN);
                    return;
                }
                if sq_repro_save_game_only() {
                    append_autoload_debug(format_args!(
                        "sq-repro: OptionSetting opened window=0x{option_setting:x} -> TO_SAVE_GAME (LB, A to enter the Quit Game tab and activate the direct Save Game row)"
                    ));
                } else if sq_repro_profile_back_mode() {
                    append_autoload_debug(format_args!(
                        "sq-repro: OptionSetting opened window=0x{option_setting:x} -> PROFILE_BACK (LB, DOWN, A to activate Load Profile, then B/back before loading)"
                    ));
                } else {
                    append_autoload_debug(format_args!(
                        "sq-repro: OptionSetting opened window=0x{option_setting:x} (quit submenu) -> TO_PROFILE (LB, DOWN, A to activate the cloned Load-Profile row)"
                    ));
                }
                set_pad(0);
                if sq_repro_profile_back_mode() {
                    SQ_REPRO_TAB_RETURN_PHASE.store(0, Ordering::SeqCst);
                    SQ_REPRO_TAB_RETURN_MAX_TAB.store(0, Ordering::SeqCst);
                    sq_repro_transition(SQ_REPRO_STATE_PROFILE_BACK_BASELINE);
                } else {
                    sq_repro_transition(SQ_REPRO_STATE_TO_PROFILE);
                }
                return;
            }
            let (btn, holding) = sq_repro_edges(tick, &[XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_A]);
            if holding {
                sq_repro_waiting_once("TO_SYSTEM: UP+A issued, waiting for 02_040_OptionSetting");
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_TAB_RETURN => {
            // Drive tabs off the OPTIONSETTING_CURRENT_TAB feedback. Phase 0: RB (right) until the tab
            // stops increasing (last/Quit tab reached). Phase 1: LB (left) until tab 0 (Game Options).
            // Phase 2: hold neutral and dwell so the pane-visibility oracle samples the (blank) tab 0,
            // then DONE (releases the input block). One clean edge per INJECT_NAV_CYCLE.
            let cur = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
            let phase = SQ_REPRO_TAB_RETURN_PHASE.load(Ordering::SeqCst);
            let pulse = |b: u16| {
                if (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN {
                    b
                } else {
                    0
                }
            };
            match phase {
                0 => {
                    let max = SQ_REPRO_TAB_RETURN_MAX_TAB.load(Ordering::SeqCst);
                    if cur != usize::MAX && cur > max {
                        SQ_REPRO_TAB_RETURN_MAX_TAB.store(cur, Ordering::SeqCst);
                        SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst); // reset stall timer on progress
                        set_pad(pulse(XINPUT_GAMEPAD_RIGHT_SHOULDER));
                    } else if tick >= SQ_REPRO_TAB_RETURN_STALL_TICKS && max > 0 {
                        append_autoload_debug(format_args!(
                            "sq-repro: TAB_RETURN reached last tab={max} (stalled RIGHT) -> phase 1 (LB back to Game Options)"
                        ));
                        SQ_REPRO_TAB_RETURN_PHASE.store(1, Ordering::SeqCst);
                        SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
                        set_pad(0);
                    } else {
                        set_pad(pulse(XINPUT_GAMEPAD_RIGHT_SHOULDER));
                    }
                }
                1 => {
                    if cur == 0 {
                        append_autoload_debug(format_args!(
                            "sq-repro: TAB_RETURN back on Game Options (tab 0) -> phase 2 dwell {SQ_REPRO_TAB_RETURN_DWELL_TICKS} ticks (pane oracle samples the blank)"
                        ));
                        SQ_REPRO_TAB_RETURN_PHASE.store(2, Ordering::SeqCst);
                        SQ_REPRO_TAB_RETURN_DWELL_START.store(tick, Ordering::SeqCst);
                        set_pad(0);
                    } else {
                        set_pad(pulse(XINPUT_GAMEPAD_LEFT_SHOULDER));
                    }
                }
                _ => {
                    let start = SQ_REPRO_TAB_RETURN_DWELL_START.load(Ordering::SeqCst);
                    set_pad(0);
                    if tick.saturating_sub(start) >= SQ_REPRO_TAB_RETURN_DWELL_TICKS {
                        append_autoload_debug(format_args!(
                            "sq-repro: TAB_RETURN dwell complete on tab={cur}; SELF-DRIVE COMPLETE; releasing block (check oracle_optionsetting_real_blank_detected_count / _pane_fix_applied)"
                        ));
                        SQ_REPRO_STATE.store(SQ_REPRO_STATE_DONE, Ordering::SeqCst);
                    }
                }
            }
        }
        SQ_REPRO_STATE_PROFILE_BACK_BASELINE => {
            let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
            let cur = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
            if option != 0 && cur < OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT {
                sq_repro_profile_back_record_baseline(option, cur);
            }
            let max = SQ_REPRO_TAB_RETURN_MAX_TAB.load(Ordering::SeqCst);
            let pulse = |b: u16| {
                if (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN {
                    b
                } else {
                    0
                }
            };
            if cur != usize::MAX && cur > max {
                SQ_REPRO_TAB_RETURN_MAX_TAB.store(cur, Ordering::SeqCst);
                SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
                set_pad(pulse(XINPUT_GAMEPAD_RIGHT_SHOULDER));
            } else if tick >= SQ_REPRO_TAB_RETURN_STALL_TICKS && max > 0 {
                let mask = SQ_REPRO_PROFILE_BACK_BASELINE_MASK.load(Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "sq-repro: PROFILE_BACK baseline reached last tab={max} mask=0x{mask:x}; return to Game Options, then use known LB+DOWN+A Load Profile sequence"
                ));
                SQ_REPRO_TAB_RETURN_PHASE.store(0, Ordering::SeqCst);
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_PROFILE_BACK_OPEN);
            } else {
                set_pad(pulse(XINPUT_GAMEPAD_RIGHT_SHOULDER));
            }
        }
        SQ_REPRO_STATE_PROFILE_BACK_OPEN => {
            let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            if profile != 0 {
                SQ_REPRO_PROFILE_BACK_OPENED.store(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "sq-repro: ProfileSelect opened window=0x{profile:x} -- PROFILE_BACK: send B/back, no slot select, no load"
                ));
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_PROFILE_BACK);
                return;
            }
            let cur = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
            let phase = SQ_REPRO_TAB_RETURN_PHASE.load(Ordering::SeqCst);
            if phase == 0 {
                if cur == 0 {
                    append_autoload_debug(format_args!(
                        "sq-repro: PROFILE_BACK_OPEN returned to Game Options tab 0; issue known LB+DOWN+A Load Profile sequence"
                    ));
                    SQ_REPRO_TAB_RETURN_PHASE.store(1, Ordering::SeqCst);
                    SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
                    set_pad(0);
                } else {
                    let btn = if (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN {
                        XINPUT_GAMEPAD_LEFT_SHOULDER
                    } else {
                        0
                    };
                    if tick % (INJECT_NAV_CYCLE * 8) == 0 {
                        append_autoload_debug(format_args!(
                            "sq-repro: PROFILE_BACK_OPEN returning from baseline tab={cur} to Game Options with LB pulses"
                        ));
                    }
                    set_pad(btn);
                }
                return;
            }
            let (btn, holding) = sq_repro_edges(
                tick,
                &[
                    XINPUT_GAMEPAD_LEFT_SHOULDER,
                    XINPUT_GAMEPAD_DPAD_DOWN,
                    XINPUT_GAMEPAD_A,
                ],
            );
            if holding {
                sq_repro_waiting_once(
                    "PROFILE_BACK_OPEN: LB+DOWN+A issued from Game Options, waiting for 05_010_ProfileSelect",
                );
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_TO_PROFILE => {
            if sq_repro_save_game_only() {
                let action_count = SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT.load(Ordering::SeqCst);
                let save_count = SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT.load(Ordering::SeqCst);
                let close_count = SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT.load(Ordering::SeqCst);
                if action_count != 0 && save_count != 0 && close_count >= 2 {
                    append_autoload_debug(format_args!(
                        "sq-repro: Save Game row completed action_count={action_count} save_count={save_count} close_count={close_count}; SELF-DRIVE COMPLETE; releasing block"
                    ));
                    set_pad(0);
                    SQ_REPRO_STATE.store(SQ_REPRO_STATE_DONE, Ordering::SeqCst);
                    return;
                }
                let (btn, holding) =
                    sq_repro_edges(tick, &[XINPUT_GAMEPAD_LEFT_SHOULDER, XINPUT_GAMEPAD_A]);
                if holding {
                    sq_repro_waiting_once(
                        "TO_SAVE_GAME: LB+A issued, waiting for Save Game action/save/close telemetry",
                    );
                }
                set_pad(btn);
                return;
            }
            let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            if profile != 0 {
                if sq_repro_profile_back_mode() {
                    SQ_REPRO_PROFILE_BACK_OPENED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "sq-repro: ProfileSelect opened window=0x{profile:x} -- PROFILE_BACK: send B/back, no slot select, no load"
                    ));
                    set_pad(0);
                    sq_repro_transition(SQ_REPRO_STATE_PROFILE_BACK);
                    return;
                }
                if sq_repro_pause_at_menu() {
                    // PAUSE-AT-MENU: the character-load menu is open -- stop HERE. No cursor move,
                    // no slot pick, no load. DONE stops the pad fabrication and releases the input
                    // block (sq_repro_actively_driving -> false), so the user's real input goes
                    // live on the open ProfileSelect. The latch is the run's PASS oracle.
                    SQ_REPRO_PAUSED_AT_PROFILE_SELECT.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "sq-repro: ProfileSelect opened window=0x{profile:x} -- PAUSE-AT-MENU: autopilot DONE at the character-load menu (no pick, no load); input block releases, game left running"
                    ));
                    set_pad(0);
                    SQ_REPRO_STATE.store(SQ_REPRO_STATE_DONE, Ordering::SeqCst);
                    return;
                }
                append_autoload_debug(format_args!(
                    "sq-repro: ProfileSelect opened window=0x{profile:x} (cloned Load-Profile row activated) -> TO_SLOT (move cursor off the current save)"
                ));
                set_pad(0);
                SQ_REPRO_INITIAL_CURSOR.store(usize::MAX, Ordering::SeqCst);
                sq_repro_transition(SQ_REPRO_STATE_TO_SLOT);
                return;
            }
            let (btn, holding) = sq_repro_edges(
                tick,
                &[
                    XINPUT_GAMEPAD_LEFT_SHOULDER,
                    XINPUT_GAMEPAD_DPAD_DOWN,
                    XINPUT_GAMEPAD_A,
                ],
            );
            if holding {
                sq_repro_waiting_once(
                    "TO_PROFILE: LB+DOWN+A issued, waiting for 05_010_ProfileSelect",
                );
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_PROFILE_BACK => {
            let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            let restore_count = SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT.load(Ordering::SeqCst);
            let baseline = SQ_REPRO_PROFILE_BACK_RESTORE_BASELINE.load(Ordering::SeqCst);
            let direct_visible =
                SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_REAPPLY_COUNT.load(Ordering::SeqCst);
            if profile == 0 && restore_count > baseline && direct_visible != 0 {
                SQ_REPRO_PROFILE_BACK_RESTORE_COUNT.store(restore_count, Ordering::SeqCst);
                SQ_REPRO_PROFILE_BACK_MISMATCH_MASK.fetch_or(
                    SQ_REPRO_PROFILE_BACK_VISIBLE_ORACLE_MISSING_MASK,
                    Ordering::SeqCst,
                );
                append_autoload_debug(format_args!(
                    "sq-repro: PROFILE_BACK observed ProfileSelect closed + restore_count {baseline}->{restore_count} + direct-visible reapply count={direct_visible}; FAIL-CLOSED visible row-content oracle missing (native backing-table hashes are insufficient)"
                ));
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_PROFILE_BACK_TO_GAME_TAB);
                return;
            }
            let (btn, holding) = sq_repro_edges(tick, &[XINPUT_GAMEPAD_B]);
            if holding {
                sq_repro_waiting_once(
                    "PROFILE_BACK: B/back issued, waiting for ProfileSelect close + restore",
                );
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_PROFILE_BACK_TO_GAME_TAB => {
            let cur = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
            let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
            if option != 0 && cur < OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT {
                sq_repro_profile_back_verify_tab(option, cur);
            }
            let load_armed = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
                || SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.load(Ordering::SeqCst) != 0
                || SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.load(Ordering::SeqCst) != 0
                || SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.load(Ordering::SeqCst) != 0;
            let baseline_mask = SQ_REPRO_PROFILE_BACK_BASELINE_MASK.load(Ordering::SeqCst);
            let verify_mask = SQ_REPRO_PROFILE_BACK_VERIFY_MASK.load(Ordering::SeqCst);
            let mismatch_mask = SQ_REPRO_PROFILE_BACK_MISMATCH_MASK.load(Ordering::SeqCst);
            let verified_all = baseline_mask != 0 && (verify_mask & baseline_mask) == baseline_mask;
            if cur == 0 && verified_all && tick >= SQ_REPRO_TAB_RETURN_DWELL_TICKS {
                SQ_REPRO_PROFILE_BACK_FINAL_TAB.store(cur, Ordering::SeqCst);
                let pass = !load_armed && mismatch_mask == 0;
                SQ_REPRO_PROFILE_BACK_DONE.store(pass as usize, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "sq-repro: PROFILE_BACK complete final_tab={cur} load_armed={load_armed} baseline_mask=0x{baseline_mask:x} verify_mask=0x{verify_mask:x} mismatch_mask=0x{mismatch_mask:x} pass={pass}; SELF-DRIVE COMPLETE; releasing block"
                ));
                set_pad(0);
                SQ_REPRO_STATE.store(SQ_REPRO_STATE_DONE, Ordering::SeqCst);
                return;
            }
            let btn = if cur == 0 {
                0
            } else if (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN {
                XINPUT_GAMEPAD_LEFT_SHOULDER
            } else {
                0
            };
            if tick % (INJECT_NAV_CYCLE * 8) == 0 {
                append_autoload_debug(format_args!(
                    "sq-repro: PROFILE_BACK_TO_GAME_TAB current_tab={cur}; pulsing LB until Game Options tab 0 then dwell baseline_mask=0x{baseline_mask:x} verify_mask=0x{verify_mask:x} mismatch_mask=0x{mismatch_mask:x}"
                ));
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_TO_SLOT => {
            // Drive the ProfileSelect cursor to THIS switch's EXPLICIT target slot (not "one off
            // current"), so switch #2 lands on a real, distinct character regardless of which slot the
            // prior reload made current. DOWN increments the cursor index, UP decrements (verified:
            // switch #1 UP moved cursor 5->4). Recompute the direction each frame so an overshoot
            // self-corrects. Stop + CONFIRM when the cursor equals the target.
            let dialog = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            let cursor = if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
            } else {
                -1
            };
            let target = sq_repro_target_slot();
            if cursor < 0 {
                // ProfileSelect not fully built yet; hold neutral.
                set_pad(0);
                return;
            }
            if cursor == target {
                append_autoload_debug(format_args!(
                    "sq-repro: ProfileSelect cursor={cursor} == target_slot={target} (switch #{}) -> CONFIRM (A)",
                    SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst) + 1
                ));
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_CONFIRM);
                return;
            }
            let dir = if cursor < target {
                XINPUT_GAMEPAD_DPAD_DOWN
            } else {
                XINPUT_GAMEPAD_DPAD_UP
            };
            // Step one clean edge per INJECT_NAV_CYCLE toward the target (tap then gap = one cursor
            // step); keep stepping until cursor == target. No fixed edge budget -- advance on the
            // observed cursor value, so a missed step just re-issues next cycle.
            let btn = if (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN {
                dir
            } else {
                INJECT_NAV_NO_BUTTONS
            };
            if tick % (INJECT_NAV_CYCLE * 8) == 0 {
                append_autoload_debug(format_args!(
                    "sq-repro: TO_SLOT stepping cursor={cursor} -> target_slot={target} dir=0x{dir:04x}"
                ));
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_CONFIRM => {
            // The user's pick: ONE A on the highlighted slot. The activate hook direct-arms the
            // save-safe switch and native cancel-closes ProfileSelect (the product path -- no confirm
            // MessageBox exists; the suppression eats it before UI allocation). DONE is gated on the
            // arm being OBSERVED: the arm advances SYSTEM_QUIT_QUICKLOAD_PHASE past IDLE (phase is
            // reliably IDLE on CONFIRM entry -- continue_confirm resets it at each reload's commit,
            // long before WAIT_RELOAD's load_done+settle gates admit the next switch). The legacy
            // confirm-count predicate is kept as a fallback for the direct-arm-did-not-take native
            // forward (confirm box -> OK -> load-job chain). On either signal, release the pad; the
            // native pump drives the return-title -> autoload.
            let armed = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE;
            if armed || sq_repro_confirm_count() > SQ_REPRO_CONFIRM_BASELINE.load(Ordering::SeqCst)
            {
                let switch_index = SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst);
                let more = switch_index + 1 < sq_repro_target_switches();
                append_autoload_debug(format_args!(
                    "sq-repro: switch #{}/{} load CONFIRMED via {} (confirmed_block={} confirmed_allow={} activate={} baseline={}). {}",
                    switch_index + 1,
                    sq_repro_target_switches(),
                    if armed {
                        "direct-arm"
                    } else {
                        "OK-confirm chain"
                    },
                    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.load(Ordering::SeqCst),
                    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.load(Ordering::SeqCst),
                    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.load(Ordering::SeqCst),
                    SQ_REPRO_CONFIRM_BASELINE.load(Ordering::SeqCst),
                    if more {
                        "native pump drives return-title + reload; then WAIT_RELOAD -> next switch"
                    } else {
                        "SELF-DRIVE COMPLETE; releasing block, native pump drives return-title + autoload"
                    }
                ));
                set_pad(0);
                if more {
                    sq_repro_transition(SQ_REPRO_STATE_WAIT_RELOAD);
                } else {
                    SQ_REPRO_STATE.store(SQ_REPRO_STATE_DONE, Ordering::SeqCst);
                }
                return;
            }
            let (btn, holding) = sq_repro_edges(tick, &[XINPUT_GAMEPAD_A]);
            if holding {
                sq_repro_waiting_once(
                    "CONFIRM: A (pick) issued, waiting for direct-arm/load-confirm",
                );
            }
            set_pad(btn);
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
            let player_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
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
            let loading = !base_ok || !load_done || fake_cover;
            if deser < expected_deser || !player_up || loading {
                // Still tearing down / at title / streaming: hold the settle clock at 0 so it starts
                // only when the NEW world is up AND interactive (not a loading-screen false positive).
                SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
                // Periodic GATE dump (er-effects-rs-qwj): this state once stalled with switch #1
                // stable and fresh-deser == expected, so one of these gates was lying. Name the
                // culprit with data, not a single opaque waiting line.
                let waited = SQ_REPRO_WAIT_RELOAD_FRAMES.fetch_add(1, Ordering::SeqCst);
                if waited % SQ_REPRO_WAIT_RELOAD_LOG_EVERY == 0 {
                    append_autoload_debug(format_args!(
                        "sq-repro: WAIT_RELOAD gates (switch #{}/{} waited_frames={waited}): fresh_deser={deser}/{expected_deser} player_up={player_up} load_done={load_done} fake_cover={fake_cover}",
                        switch_index + 1,
                        sq_repro_target_switches()
                    ));
                }
                return;
            }
            if tick >= SQ_REPRO_WORLD_SETTLE_TICKS {
                let next = switch_index + 1;
                SQ_REPRO_SWITCH_INDEX.store(next, Ordering::SeqCst);
                sq_repro_begin_switch();
                append_autoload_debug(format_args!(
                    "sq-repro: switch #{}/{} reload committed (fresh_deser={deser}) + new world settled -> arming switch #{}/{} target_slot={}; OPEN_MENU",
                    switch_index + 1,
                    sq_repro_target_switches(),
                    next + 1,
                    sq_repro_target_switches(),
                    sq_repro_target_slot()
                ));
                sq_repro_transition(SQ_REPRO_STATE_OPEN_MENU);
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

unsafe fn system_quit_arm_quickload_autoload(selected_slot: i32, source: &str) {
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
    // Re-arm the continue_confirm guard's one-shot: the upcoming clean-title confirm must drive a
    // fresh deserialize of THIS switch's picked slot before it streams (the hook itself is installed
    // unconditionally at attach; see install_system_quit_continue_confirm_hook).
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(0, Ordering::SeqCst);
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
    // PORTRAIT RETARGET (user 2026-07-03): the user just confirmed a NEW character for load, so the
    // loading-screen portrait should render THAT character, not the one still resident (ac0). Make it
    // before-break: retarget the spare/render to the selected slot (portrait_target_slot now returns
    // it) and RE-ENGAGE the drive (clear the per-window freeze) so the new model renders + gets its
    // depth mask -- but do NOT touch LOADING_BG_PORTRAIT_RGBA / PROFILE_HAVE_KEYED_FRAME, so the prior
    // masked head keeps displaying until the new model's first KEYED frame replaces it (no opaque
    // flash, no blank). Clear the stale spare candidate (captured for the old character before this
    // confirm) so the teardown-spare re-targets the new slot, and drop the depth-mask cache so the new
    // silhouette is computed fresh rather than bridged from the old head.
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
    append_autoload_debug(format_args!(
        "loading-portrait: RETARGET to selected slot {selected_slot} at confirm (make-before-break: drive re-engaged, prior masked head holds until the new keyed frame; source={source})"
    ));
    rearm_boot_progress_for_own_menu_load(selected_slot, source);
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED, Ordering::SeqCst);
    OWN_STEPPER_SLOT.store(selected_slot, Ordering::SeqCst);
    PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU, Ordering::SeqCst);
    TFC_CONTINUE_FIRED.store(0, Ordering::SeqCst);
    TFC_LOAD_VEC_WAIT_TICKS.store(0, Ordering::SeqCst);
    OWN_STEPPER_MENU_OPENED.store(OWN_STEPPER_MENU_OPENED_NO, Ordering::SeqCst);
    TITLE_ACCEPT_BYTE_GATE_FIRED.store(false, Ordering::SeqCst);
    TITLE_OWNER_PTR.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    TITLE_OWNER_SCAN_COUNTDOWN.store(TITLE_OWNER_SCAN_COUNTDOWN_READY, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(false, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_POST_RETURN_TITLE_FIRED.store(0, Ordering::SeqCst);
    PROFILE_REFRESH_KICKED.store(0, Ordering::SeqCst);
    PORTRAIT_RENDER_WINDOW_DONE.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(
        SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
        Ordering::SeqCst,
    );
    append_autoload_debug(format_args!(
        "system-quit-quickload: armed product Continue autoload selected_slot={selected_slot} source={source}; will direct-submit native return-title chain once ProfileSelect closes system_dialog=0x{system_dialog:x}"
    ));
}

pub(crate) unsafe extern "system" fn system_quit_gaitem_finalize_hook(gaitem: usize) {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        let skips = SYSTEM_QUIT_GAITEM_FINALIZE_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp finalize SKIPPED during return-title transition #{skips} phase={phase} gaitem=0x{gaitem:x}; avoids post-deserialize singleton-state assert while native return-title job advances"
        ));
        return;
    }
    SYSTEM_QUIT_GAITEM_FINALIZE_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_GAITEM_FINALIZE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp finalize trampoline unset phase={phase} gaitem=0x{gaitem:x}; fail-closed skip"
        ));
        return;
    }
    let original: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { original(gaitem) };
}

pub(crate) unsafe extern "system" fn system_quit_gaitem_lookup_hook(
    gaitem: usize,
    out_handle: usize,
    in_handle: usize,
) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        if out_handle != 0 && out_handle != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *(out_handle as *mut u32) = 0 };
        }
        let empties = SYSTEM_QUIT_GAITEM_LOOKUP_EMPTY_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if empties <= 16 || empties % 64 == 0 {
            let input = unsafe { safe_read_i32(in_handle) }.unwrap_or(0) as u32;
            append_autoload_debug(format_args!(
                "system-quit-quickload: CSGaitemImp lookup EMPTIED during return-title transition #{empties} phase={phase} gaitem=0x{gaitem:x} out=0x{out_handle:x} in=0x{in_handle:x} input=0x{input:x}; avoids ChrAsm equipment lookup assert while stream remains consumed"
            ));
        }
        return out_handle;
    }
    SYSTEM_QUIT_GAITEM_LOOKUP_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_GAITEM_LOOKUP_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp lookup trampoline unset phase={phase} gaitem=0x{gaitem:x}; returning empty"
        ));
        if out_handle != 0 && out_handle != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *(out_handle as *mut u32) = 0 };
        }
        return out_handle;
    }
    let original: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(gaitem, out_handle, in_handle) }
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
    unsafe { original(slot) }
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
                    std::mem::forget(hook);
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
        if n <= 8 || n % 120 == 0 {
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
                    std::mem::forget(hook);
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
    if switch_active && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0 {
        let selected = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
        let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
        if world_up {
            // A title-flow confirm while the old world is still up is not a state we ever drive;
            // never deserialize into a live world (that is the crash the whole switch avoids).
            // Forward and log loudly -- the in-world load guards protect the load paths.
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm called while OLD WORLD STILL UP phase={phase} selected={selected} shim=0x{shim:x} -- forwarding WITHOUT fresh deserialize (unexpected caller)"
            ));
        } else if selected >= TITLE_PROFILE_SLOT_COUNT {
            let n = SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm BLOCKED #{n} -- switch active (phase={phase}) but no valid picked slot ({selected}); refusing to stream stale pre-switch state"
            ));
            return 0;
        } else {
            let slot = selected as i32;
            let base = game_rva(0).unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let gm = game_man_ptr_or_null();
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm intercepted at clean title phase={phase} -> restore gaitem singleton + fresh feed-deserialize of PICKED slot {slot} before stream (shim=0x{shim:x})"
            ));
            // Release char#1's leaked gaitems back to the free-queue at this clean title (player
            // absent) BEFORE the reload deserialize, else char#2's deserialize exhausts the queue
            // and OOB-dispatches gaitemInsTable[-1] (the AV at live 0x67141a). Native per-item
            // release; declines fail-closed if the singleton looks wrong (then the deserialize may
            // still crash, but we never sweep a bogus pointer).
            if base != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { own_load_reset_gaitem_singleton(base) };
            }
            if base != TITLE_OWNER_SCAN_START_ADDRESS
                && unsafe { own_load_feed_deserialize(base, gm, slot) }
            {
                SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(1, Ordering::SeqCst);
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
                SYSTEM_QUIT_QUICKLOAD_PHASE
                    .store(SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE, Ordering::SeqCst);
                // CLEAR the stale in-world load arm. product-core armed GameMan+0xb78 = slot MANY
                // times before this confirm (title.rs, phase 3-4). Phase -> IDLE stops FURTHER arming
                // but leaves b78 = slot RESIDENT; once our SetState5 world comes up, the in-world
                // MoveMapStep loader reads that stale b78 and fires a REDUNDANT second load of the same
                // slot -> a second CSGaitemImp::Deserialize with the free-queue already populated by
                // our load -> the 0x67141a exhaustion crash (observed +41705ms). With phase IDLE the
                // in-world guards are inert, so clear b78 to -1 (native "no requested slot") ourselves.
                if gm != TITLE_OWNER_SCAN_START_ADDRESS {
                    unsafe {
                        *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *mut i32) =
                            OWN_STEPPER_SLOT_NONE;
                    }
                }
                // CLEAR the return-title "rebuild the title" request flags the final functor set for
                // this switch's teardown. They are LEVEL flags nothing resets, so once the reloaded
                // world comes up the still-set menuData+0x5d re-requests the quit-to-title
                // (GameMan.save_requested flips true again ~3.6s later, proven by gm-snap) and bounces
                // the freshly-loaded world back to the title. The teardown they were needed for is done
                // by now (we are at the clean-title Continue), so undo them.
                let menu_man = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
                    .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if menu_man != TITLE_OWNER_SCAN_START_ADDRESS
                    && unsafe { is_heap_aligned_ptr(menu_man) }
                {
                    if let Some(menu_data) =
                        unsafe { safe_read_usize(menu_man + CS_MENU_MAN_MENU_DATA_OFFSET) }
                    {
                        if menu_data != TITLE_OWNER_SCAN_START_ADDRESS
                            && unsafe { is_heap_aligned_ptr(menu_data) }
                        {
                            unsafe {
                                *((menu_data + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET)
                                    as *mut u8) = 0;
                            }
                        }
                    }
                }
                unsafe { *((base + RETURN_TITLE_REBUILD_FLAG_DAT_RVA) as *mut u8) = 0 };
                // Also clear GameMan.save_requested defensively (typed): the return-title REQUEST set
                // it for the teardown; a residual true would drive an immediate quit-save on the reload.
                // AND clear GameMan.warp_requested: the fresh full deserialize we just ran (native
                // parser 0x67b290 = dump FUN_14067b380) UNCONDITIONALLY sets warp_requested=true as a
                // "warp reload pending" flag. On the normal in-world load the MoveMapStep warp machine
                // consumes it, but our SetState5 forward is a fresh title->world stream that never does;
                // MoveMapStep::CheckReturnToTitle (dump FUN_140afa7c0) then reads warp_requested==true
                // every frame as a return-to-title trigger and bounces the freshly-loaded world back to
                // the title ~4s later (proven: gm-snap shows warp_requested=true for the whole reloaded
                // world vs false on the healthy boot load). warp_requested=false is the correct in-world
                // steady state, so clearing it matches the boot load and does not affect which char loads.
                if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
                    er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
                    er_save_loader::GameManSaveAccess::set_warp_requested(gm_typed, false);
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
                // consumed, and alongside phase -> IDLE, so no return-title path reads a 0 count until
                // the next switch arms (all those gates require phase != IDLE).
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
                    "system-quit-quickload: fresh picked-slot deserialize OK #{n} slot={slot} -- forwarding continue_confirm so SetState5 streams; phase -> IDLE + cleared GameMan+0xb78=-1 + cleared return-title rebuild flags (menuData+0x5d, DAT, save_requested, warp_requested) + RESET return-title one-shots (request/submit/final-functor) so the NEXT switch starts boot-fresh (er-effects-rs-qwj repeatable switching)"
                ));
            } else {
                let n = SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                append_autoload_debug(format_args!(
                    "system-quit-quickload: continue_confirm BLOCKED #{n} -- fresh deserialize of picked slot {slot} FAILED (see own-load-feed line); refusing to stream stale pre-switch state"
                ));
                return 0;
            }
        }
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
