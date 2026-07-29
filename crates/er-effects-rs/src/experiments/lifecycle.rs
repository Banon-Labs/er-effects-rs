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
pub(crate) use er_telemetry::counters::HARNESS_DISC_STABLE;
static HARNESS_DISC_PHASE: AtomicUsize = AtomicUsize::new(0); // 0 wait,1 press,2 release,3 observe,4 done
pub(crate) use er_telemetry::counters::HARNESS_DISC_PHASE_FRAME;
static HARNESS_DISC_SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// DE-GATED (deprecate-env-marker-gate-allowlists-2026-07-19): the agent-owned switch-harness
/// feasibility probe (blocks keyboard, pulses DIK_ESCAPE, logs MenuWindowJob::Run filenames) was an
/// env/marker-gated diagnostic autopilot. Env/marker feature gates are forbidden; retired (off).
pub(crate) fn switch_harness_discovery_enabled() -> bool {
    false
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

// === SAVE-FLOW state machine (save-game-flow WP1 + WP2 + WP3, 2026-07-28) ===
// Drives the System->Quit "Save Game" row's confirm chain and CLOSE-THEN-FIRE commit.
// Stage map lives on `er_telemetry::counters::SAVE_FLOW_STAGE` (oracle_save_flow_stage):
// 0 IDLE, 1 BOX1_WAIT, 2 BOX2_WAIT, 3 DEST_BROWSE, 4 BOX3_WAIT, 5 CLOSING_ABORT,
// 6 CLOSING_COMMIT, 7 FIRE_GATE_WAIT, 8 COMMIT_WAIT.
//
// WP2 added the confirm chain: the row press submits Box1 ("Are you sure you want to
// save?", default No) inline on the menu thread; the tick POLLS the box result (pure
// reads) and, on Yes, stages Box2 ("Overwrite your loaded save?", default Yes) for the
// menu pump to submit. Box2 Yes stages the commit, No opens the WP3 destination browser,
// cancel aborts. Both terminal paths run the proven close sequence (OptionSetting
// immediately, IngameTop deferred 2 frames), and only once the menus are closed AND the
// RAM gates are green does the tick arm the one-shot er-save-suppress bypass and fire the
// FORCED (throttle-skipping) native save request pair. The tick only reads and decides;
// all menu mutation stays on the paths that already own it, except the window CLOSE,
// which the shipping deferred IngameTop close already performs from this same game task.
//
// WP3 added the destination browser (stage 3) and its overwrite confirm (stage 4). A
// chosen destination is committed by ARMING a scoped write-open redirect just before the
// fire, so the native writer's own container write lands on the destination while the
// loaded save is only read; stage 8 verifies both files before returning to IDLE.

/// Transition helper: swap the stage, reset the per-stage tick counter, log the edge.
fn save_flow_enter_stage(stage: usize, reason: &str) {
    let prev = SAVE_FLOW_STAGE.swap(stage, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-flow: stage {prev} -> {stage} ({reason})"
    ));
}

/// Per-frame save-flow driver. Called from the game task immediately AFTER
/// `system_quit_save_game_deferred_close_tick`, so the frame the deferred IngameTop
/// close drains is the same frame stage 6 observes "menus closed".
pub(crate) unsafe fn save_flow_tick() {
    let stage = SAVE_FLOW_STAGE.load(Ordering::SeqCst);
    if stage == SAVE_FLOW_STAGE_IDLE {
        return;
    }
    let ticks = SAVE_FLOW_STAGE_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    match stage {
        SAVE_FLOW_STAGE_BOX1_WAIT => unsafe {
            save_flow_box_wait_tick(SAVE_FLOW_BOX_CONFIRM_SAVE, ticks)
        },
        SAVE_FLOW_STAGE_BOX2_WAIT => unsafe {
            save_flow_box_wait_tick(SAVE_FLOW_BOX_OVERWRITE_LOADED, ticks)
        },
        SAVE_FLOW_STAGE_DEST_BROWSE => unsafe { save_flow_dest_browse_tick(ticks) },
        SAVE_FLOW_STAGE_BOX3_WAIT => unsafe {
            save_flow_box_wait_tick(SAVE_FLOW_BOX_OVERWRITE_FILE, ticks)
        },
        SAVE_FLOW_STAGE_CLOSING_ABORT | SAVE_FLOW_STAGE_CLOSING_COMMIT => {
            // The close sequence itself is owned by system_quit_save_game_close_menus
            // (OptionSetting now) + the deferred-close tick (IngameTop, 2 frames). Menus are
            // closed once the deferral countdown has drained; when no top window was deferred
            // the countdown was never armed and this advances on the first tick.
            if SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.load(Ordering::SeqCst) == 0 {
                if stage == SAVE_FLOW_STAGE_CLOSING_COMMIT {
                    save_flow_enter_stage(SAVE_FLOW_STAGE_FIRE_GATE_WAIT, "menus closed");
                } else {
                    SAVE_FLOW_ABORT_COUNT.fetch_add(1, Ordering::SeqCst);
                    save_flow_box_clear();
                    save_dest_reset("aborted without writing");
                    save_flow_enter_stage(
                        SAVE_FLOW_STAGE_IDLE,
                        "menus closed; aborted without writing",
                    );
                }
            }
        }
        SAVE_FLOW_STAGE_FIRE_GATE_WAIT => unsafe { save_flow_fire_gate_tick(ticks) },
        SAVE_FLOW_STAGE_COMMIT_WAIT => save_flow_commit_wait_tick(ticks),
        _ => {
            // Every stage id in the map is handled above, so a value here is state corruption.
            // Reset loudly, never wedge.
            append_autoload_debug(format_args!(
                "save-flow: tick saw unknown stage {stage}; resetting to IDLE"
            ));
            save_flow_box_clear();
            save_dest_reset("unknown stage");
            save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "unknown stage");
        }
    }
}

/// Stages 1/2 BOX*_WAIT: the confirm box is (or is becoming) visible. PURE READS -- every
/// menu mutation this decides is staged for the owning thread (`SAVE_FLOW_SUBMIT_BOX_PENDING`
/// for the next box) or runs through the proven close sequence.
///
/// There is deliberately NO timeout on the user's decision; the only timeout is on the BUILD,
/// i.e. the box never appearing at all, which means the recipe failed and waiting is pointless.
unsafe fn save_flow_box_wait_tick(box_id: usize, ticks: usize) {
    let Some(decision) = (unsafe { save_flow_box_decision(box_id) }) else {
        // The ONLY timeout in this stage covers the box never BECOMING visible: either the
        // menu pump never consumed the submit pending, or the submitted job never reached the
        // MessageBoxDialog builder. Both mean waiting longer cannot help.
        if SAVE_FLOW_BOX_DIALOG.load(Ordering::SeqCst) == 0
            && ticks >= SAVE_FLOW_BOX_BUILD_TIMEOUT_TICKS
        {
            let pending = SAVE_FLOW_SUBMIT_BOX_PENDING.load(Ordering::SeqCst);
            SAVE_FLOW_BOX_BUILD_TIMEOUT_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: {} BUILD TIMEOUT after {ticks} ticks (submit_pending={pending}) -- the confirm box never became visible; ending the flow, the user's save did NOT happen",
                save_flow_box_label(box_id)
            ));
            save_flow_box_clear();
            if box_id == SAVE_FLOW_BOX_OVERWRITE_FILE {
                // Box3 sits OVER the destination browser: tear the picker down first (its close
                // restores the user's rows and re-shows the System windows) and let the stage-3
                // abort path close the menus once the window is gone.
                let picker = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
                save_dest_clear_target("box3 build timeout");
                unsafe { save_flow_close_dest_picker_from_tick(picker, "box3_build_timeout") };
                save_flow_enter_stage(SAVE_FLOW_STAGE_DEST_BROWSE, "box3 build timeout");
            } else {
                unsafe { save_flow_close_menus_from_tick("box_build_timeout", false) };
            }
        }
        return;
    };
    // UNDECIDABLE outranks the per-box routing: the box was freed/reused, or it reported an
    // answer we could not map. That is a FAILURE of ours, not a user "No" -- it never advances
    // toward a write, and it is reported as a failure so a run can tell the two apart.
    if decision == SaveFlowDecision::Undecidable {
        append_autoload_debug(format_args!(
            "save-flow: {} could NOT be resolved (undecidable) -- closing back to the world with NOTHING written. This is a save-flow FAILURE, not the user declining; see the preceding save-flow-box line for the fields that were read",
            save_flow_box_label(box_id)
        ));
        if box_id == SAVE_FLOW_BOX_OVERWRITE_FILE {
            // Box3 sits over the destination browser: tear the picker down first, exactly like
            // its build-timeout path, so the abort does not close menus under a live window.
            let picker = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            save_dest_clear_target("box3 undecidable");
            unsafe { save_flow_close_dest_picker_from_tick(picker, "box3_undecidable") };
            save_flow_enter_stage(SAVE_FLOW_STAGE_DEST_BROWSE, "box3 undecidable");
        } else {
            unsafe { save_flow_close_menus_from_tick("box_undecidable", false) };
        }
        return;
    }
    match (box_id, decision) {
        (SAVE_FLOW_BOX_CONFIRM_SAVE, SaveFlowDecision::Yes) => {
            // Menu-pump owns the submit; the tick only stages it.
            SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_OVERWRITE_LOADED, Ordering::SeqCst);
            save_flow_enter_stage(SAVE_FLOW_STAGE_BOX2_WAIT, "box1 Yes -> overwrite confirm");
        }
        (SAVE_FLOW_BOX_OVERWRITE_LOADED, SaveFlowDecision::Yes) => {
            // Commit plan: overwrite the loaded save. No destination target is set, so the fire
            // gate arms no redirect and the native writer hits its normal path.
            unsafe { save_flow_close_menus_from_tick("box2_overwrite_loaded", true) };
        }
        (SAVE_FLOW_BOX_OVERWRITE_LOADED, SaveFlowDecision::No) => {
            // "Save somewhere else": hand the destination browser open to the menu pump (staging
            // records + submitting the 05_010 job is menu-pump work) and wait in stage 3.
            SAVE_DEST_OPEN_PICKER_PENDING.store(1, Ordering::SeqCst);
            save_flow_enter_stage(
                SAVE_FLOW_STAGE_DEST_BROWSE,
                "box2 No -> choose a save destination",
            );
        }
        (SAVE_FLOW_BOX_OVERWRITE_FILE, SaveFlowDecision::Yes) => {
            // Final overwrite confirm for an existing destination file. The picker close is the
            // same native primitive the deferred IngameTop close already calls from this task.
            let picker = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            SAVE_DEST_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
            SAVE_DEST_COMMIT_PENDING.store(1, Ordering::SeqCst);
            unsafe { save_flow_close_dest_picker_from_tick(picker, "box3_overwrite_confirmed") };
            save_flow_enter_stage(
                SAVE_FLOW_STAGE_DEST_BROWSE,
                "box3 Yes -> commit to the chosen file",
            );
        }
        (SAVE_FLOW_BOX_OVERWRITE_FILE, SaveFlowDecision::No) => {
            // Declining the overwrite drops only the target: the browser is untouched, so the
            // user can pick a different destination.
            save_dest_clear_target("box3 declined");
            save_flow_enter_stage(
                SAVE_FLOW_STAGE_DEST_BROWSE,
                "box3 No -> back to the destination browser",
            );
        }
        (_, SaveFlowDecision::No) => {
            append_autoload_debug(format_args!(
                "save-flow: {} answered No/cancel -- closing back to the world, nothing will be written",
                save_flow_box_label(box_id)
            ));
            unsafe { save_flow_close_menus_from_tick("box_declined", false) };
        }
        (other, _) => {
            append_autoload_debug(format_args!(
                "save-flow: decision for unexpected box id {other}; aborting without writing"
            ));
            unsafe { save_flow_close_menus_from_tick("box_unexpected_id", false) };
        }
    }
}

/// Stage 3 DEST_BROWSE: the destination browser is opening, being browsed, or tearing down after
/// a destination was confirmed. PURE READS plus the two hand-offs the tick owns (closing the menus
/// once the picker is gone, and ending a flow whose browser never appeared).
///
/// The picker itself drives the interesting transitions from its own activation hook (menu
/// thread): a chosen destination sets `SAVE_DEST_COMMIT_PENDING` and closes the window, an
/// existing file goes to stage 4 first. Backing out of the browser clears the picker latches with
/// no commit pending, which is what this reads as "the user abandoned the save".
unsafe fn save_flow_dest_browse_tick(ticks: usize) {
    // A confirmed destination outranks every other latch: once the commit is staged the browser is
    // on its way out and only its teardown is being waited on.
    if SAVE_DEST_COMMIT_PENDING.load(Ordering::SeqCst) != 0 {
        if SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) == 0 {
            // Gone: its close already restored the user's ProfileSummary rows and re-showed the
            // System windows, which is the state the close-all sequence expects.
            SAVE_DEST_COMMIT_PENDING.store(0, Ordering::SeqCst);
            unsafe { save_flow_close_menus_from_tick("dest_commit", true) };
        } else if ticks >= SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS {
            // The browser will not go away. Nothing has been armed or fired yet, so abort rather
            // than close the root menus out from under a live window.
            SAVE_DEST_COMMIT_PENDING.store(0, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: destination browser did not tear down after {ticks} ticks -- abandoning the commit, the user's save did NOT happen"
            ));
            unsafe { save_flow_close_menus_from_tick("dest_teardown_timeout", false) };
        }
        return;
    }
    if SAVE_PICKER_DEST_MODE.load(Ordering::SeqCst) != 0 {
        // Browser is live and owns the screen; the user's decision has no timeout.
        return;
    }
    if SAVE_DEST_OPEN_PICKER_PENDING.load(Ordering::SeqCst) != 0 {
        // Staged for the menu pump but not open yet.
        if ticks >= SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS {
            SAVE_DEST_OPEN_PICKER_PENDING.store(0, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: destination browser never opened after {ticks} ticks -- ending the flow, the user's save did NOT happen"
            ));
            unsafe { save_flow_close_menus_from_tick("dest_picker_open_timeout", false) };
        }
        return;
    }
    // No browser, no pending open, no commit: the user backed out of the destination browser.
    SAVE_DEST_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
    save_dest_clear_target("destination browser abandoned");
    append_autoload_debug(format_args!(
        "save-flow: destination browser closed without choosing after {ticks} ticks -- returning to the world with nothing written"
    ));
    unsafe { save_flow_close_menus_from_tick("dest_abandoned", false) };
}

/// Native cancel-close of the destination browser from the game task. Same primitive (and same
/// task) as the shipping deferred IngameTop close; a stale/absent window is not fatal -- stage 3
/// then simply observes the picker latches already cleared.
unsafe fn save_flow_close_dest_picker_from_tick(picker_window: usize, reason: &str) {
    save_flow_box_clear();
    if picker_window < 0x10000 {
        append_autoload_debug(format_args!(
            "save-flow: destination browser close skipped (reason={reason}) -- window=0x{picker_window:x} is not live"
        ));
        return;
    }
    let closed = unsafe { system_quit_save_game_close_window(picker_window, "dest_picker_window") };
    append_autoload_debug(format_args!(
        "save-flow: destination browser close (reason={reason}) window=0x{picker_window:x} closed={closed}"
    ));
}

/// Run the close-all sequence for a decision reached on the game task. The native
/// MenuWindow close this performs is the same primitive `system_quit_save_game_deferred_close_tick`
/// already calls from this task every time the shipping Save Game row runs, so the context is
/// proven. `system_quit_save_game_close_menus` sets the destination stage itself.
unsafe fn save_flow_close_menus_from_tick(source: &str, commit: bool) {
    save_flow_box_clear();
    let dialog = SAVE_FLOW_DIALOG.load(Ordering::SeqCst);
    let closed = unsafe { system_quit_save_game_close_menus(dialog, source, commit) };
    let stage = SAVE_FLOW_STAGE.load(Ordering::SeqCst);
    if stage == SAVE_FLOW_STAGE_CLOSING_ABORT || stage == SAVE_FLOW_STAGE_CLOSING_COMMIT {
        if !closed {
            // Staged, but no owning window was latched to close. The stage still advances on
            // the drained deferral counter; log it because a Save Game flow that closes no
            // menu is not the shape this flow expects.
            append_autoload_debug(format_args!(
                "save-flow: close-all from tick source={source} commit={commit} closed no window (dialog=0x{dialog:x})"
            ));
        }
        return;
    }
    // The captured dialog went stale, so the close helper bailed before staging anything.
    // End the flow here instead of looping on a stage that can no longer advance.
    SAVE_FLOW_ABORT_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-flow: close-all from tick source={source} commit={commit} could not stage (dialog=0x{dialog:x} stale) -- ending the flow; the user's save did NOT happen"
    ));
    save_dest_reset("close-all could not stage");
    save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "close-all could not stage");
}

/// Stage 7 FIRE_GATE_WAIT: menus are closed; wait for the RAM gates proving the native
/// save orchestrator will accept and dispatch the request as ONE combined `b72 && b73`
/// submit, then arm the one-shot bypass and fire the forced request pair.
unsafe fn save_flow_fire_gate_tick(ticks: usize) {
    const HEAP_LO: usize = 0x10000;
    let Ok(base) = game_module_base() else {
        return;
    };
    let csm = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }.unwrap_or(0);
    // Failure latch first: `CSMenuMan->[0x80]+0x290` (byte) / `+0x298` (qword). Latched
    // means SaveRequest_Profile's gate FUN_14080d570 fails PERMANENTLY for the session --
    // waiting cannot help, so abort loudly instead of timing out (noise rule 3: failure
    // paths log on first occurrence; the counter is exported every telemetry cadence).
    if csm >= HEAP_LO {
        let sub =
            unsafe { safe_read_usize(csm + CS_MENU_MAN_SAVE_GATE_SUB_80_OFFSET) }.unwrap_or(0);
        if sub >= HEAP_LO {
            let l290 =
                unsafe { safe_read_u8(sub + CS_MENU_MAN_SAVE_GATE_LATCH_290_OFFSET) }.unwrap_or(0);
            let l298 = unsafe { safe_read_usize(sub + CS_MENU_MAN_SAVE_GATE_LATCH_298_OFFSET) }
                .unwrap_or(0);
            if l290 != 0 || l298 != 0 {
                SAVE_FLOW_GATE_LATCH_BLOCKED_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-flow: FIRE-GATE ABORT -- CSMenuMan[+0x80] failure latch set (+0x290=0x{l290:x} +0x298=0x{l298:x}); saves are dead for this session, NOT firing; the user's save did NOT happen"
                ));
                save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "gate latch blocked");
                return;
            }
        }
    }
    let dsm = if csm >= HEAP_LO {
        unsafe { safe_read_u8(csm + CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET) }.unwrap_or(u8::MAX)
    } else {
        u8::MAX
    };
    let gm = game_man_ptr_or_null();
    let (b80, bc4) = if gm >= HEAP_LO {
        (
            unsafe { safe_read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_i32(gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) }
                .unwrap_or(-1),
        )
    } else {
        (-1, -1)
    };
    // Green = ShouldSave's menu gate open (disableSaveMenu 0), no save/load in flight
    // (b80 == 0), and the quit chain not parked at READY (bc4 != 3, where the b72
    // effective-getter zeroes the request).
    let gates_green =
        dsm == 0 && b80 == 0 && bc4 != GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY as i32;
    if gates_green {
        // DESTINATION COMMIT (save-game-flow WP3): with a chosen destination that is not the
        // loaded save, arm the scoped write-open redirect (and the live-file snapshot that undoes
        // a leak) BEFORE the request is fired. Arming failure is terminal -- firing without it
        // would overwrite the very file the user chose not to overwrite.
        if let Some(target) = save_dest_target() {
            let Some(live) = save_dest_live_save_path() else {
                SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-flow: FIRE ABORT -- destination '{}' chosen but the loaded save path is unavailable; NOT firing",
                    target.display()
                ));
                save_dest_reset("live save path unavailable");
                save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "destination live path unavailable");
                return;
            };
            if !save_dest_target_is_live(&target, &live) && !save_dest_arm_redirect(&live, &target)
            {
                SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-flow: FIRE ABORT -- could not arm the destination redirect for '{}'; NOT firing, the user's save did NOT happen",
                    target.display()
                ));
                save_dest_reset("redirect arm failed");
                save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "destination arm failed");
                return;
            }
        }
        if !er_save_suppress::is_armed() {
            // Degraded fail-open (prologue mismatch / install failure): suppression never
            // armed, so every native save already writes normally. Fire the forced request
            // natively so the user's explicit Save Game press still saves; log once.
            append_autoload_debug(format_args!(
                "save-flow: suppression NOT armed (oracle_save_suppress_armed=0) -- degraded fail-open: firing forced native save request without a bypass token"
            ));
            unsafe { system_quit_save_game_request_save_forced() };
            if save_dest_redirect_armed() {
                // A destination write is now in flight with no bypass token to watch, so the
                // stage-8 watchdog owns the wait and the verification.
                save_flow_enter_stage(
                    SAVE_FLOW_STAGE_COMMIT_WAIT,
                    "degraded fail-open fire with a destination redirect armed",
                );
            } else {
                save_dest_reset("degraded fail-open fire");
                save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "degraded fail-open fire");
            }
            return;
        }
        if !er_save_suppress::arm_one_save_bypass() {
            // Refusal here means a token is already pending -- some earlier commit's
            // watchdog has not run yet. Abort rather than fire into an ambiguous token.
            append_autoload_debug(format_args!(
                "save-flow: FIRE ABORT -- arm_one_save_bypass refused (token already pending); the user's save did NOT happen"
            ));
            save_dest_verify_and_disarm("bypass arm refused");
            save_dest_reset("bypass arm refused");
            save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "bypass arm refused");
            return;
        }
        unsafe { system_quit_save_game_request_save_forced() };
        // Read back the request flags the forced pair must have set: b73 (system lane)
        // unconditionally, b72 (char-slot lane) iff saveSlot != -1. Both 1 => the next
        // pump dispatches ONE combined submit that consumes the token.
        let b72 = if gm >= HEAP_LO {
            unsafe { safe_read_u8(gm + GAME_MAN_ARM_FLAG_B72_OFFSET) }.map_or(-1, i32::from)
        } else {
            -1
        };
        let b73 = if gm >= HEAP_LO {
            unsafe { safe_read_u8(gm + GAME_MAN_FLAG_B73_PROBE_OFFSET) }.map_or(-1, i32::from)
        } else {
            -1
        };
        append_autoload_debug(format_args!(
            "save-flow: FIRED forced save request (throttle skipped) after {ticks} gate ticks; readback b72={b72} b73={b73}"
        ));
        save_flow_enter_stage(SAVE_FLOW_STAGE_COMMIT_WAIT, "forced request fired");
        return;
    }
    if ticks >= SAVE_FLOW_FIRE_GATE_TIMEOUT_TICKS {
        append_autoload_debug(format_args!(
            "save-flow: FIRE-GATE TIMEOUT after {ticks} ticks (disableSaveMenu={dsm} b80={b80} bc4={bc4}); aborting without firing -- the user's save did NOT happen"
        ));
        save_dest_reset("fire-gate timeout");
        save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "fire-gate timeout");
    }
}

/// Stage 8 COMMIT_WAIT: the forced request is in flight through the native pump with the
/// bypass token armed. Completion = the er-save-suppress poll watch captured the first
/// post-allow terminal status (0 = success). The watchdog expires a stranded token so a
/// failed fire can never leave a one-shot bypass armed for some later native save.
fn save_flow_commit_wait_tick(ticks: usize) {
    if let Some(status) = er_save_suppress::take_bypass_final_status() {
        if status == 0 {
            SAVE_FLOW_COMMIT_COMPLETE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: COMMIT COMPLETE -- bypassed save reported terminal status 0 (success) after {ticks} commit ticks"
            ));
        } else {
            append_autoload_debug(format_args!(
                "save-flow: COMMIT FAILED -- bypassed save reported terminal status {status} (0=success); the user's save did NOT complete"
            ));
        }
        // Destination commits are scored HERE (WP3): the write-open redirect is disarmed and both
        // files are checked -- the destination must be a fresh BND4 container of the live save's
        // size, and the live save must be byte-identical to its pre-fire snapshot.
        save_dest_verify_and_disarm("commit terminal status");
        save_dest_reset("commit terminal status");
        save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "commit terminal status");
        return;
    }
    if ticks >= SAVE_BYPASS_WATCHDOG_TICKS {
        let expired = er_save_suppress::expire_bypass_if_pending();
        append_autoload_debug(format_args!(
            "save-flow: COMMIT WATCHDOG after {ticks} ticks -- {}; the user's save did NOT happen",
            if expired {
                "one-shot bypass token was still pending and has been expired"
            } else {
                "token was consumed but no terminal status was observed"
            }
        ));
        // A destination write may still have landed (the degraded fail-open path has no token to
        // watch at all), so score it before dropping the window rather than assuming failure.
        save_dest_verify_and_disarm("commit watchdog");
        save_dest_reset("commit watchdog");
        save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "commit watchdog");
    }
}

pub(crate) fn tick_before_player_lookup(task_data: &FD4TaskData) {
    unsafe { switch_harness_discovery_tick() };
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
    if unsafe { PlayerIns::local_player_mut() }.is_ok() {
        if let Ok(base) = game_module_base() {
            unsafe { install_loadlist_init_capture_hook(base) };
        }
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
    // live-portrait overlay path, only install when a real background source exists, so a no-image run does not
    // accidentally forge the diagnostic checker behind the live portrait overlay.
    let persist_loading_bg = crate::config::persist_boot_background_to_loading_screen_enabled();
    if !portrait_overlay_enabled() || (persist_loading_bg && boot_bg_image_rgba_clone().is_some()) {
        START_LOADING_BG_REPLACE_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-loading-bg-portrait".to_owned())
                .spawn(install_loading_bg_replace_bind_hook);
        });
    }
    // er-effects-rs-jsm PIVOT: suppress the native loading tips (our overlay renders player-stats text
    // instead). Install at ATTACH -- BEFORE the KnowledgeLoadingScreen ctor's one-shot initial tip (~15s),
    // else the first tip is already set and only later cycles are suppressed. Live portrait overlay path only.
    if portrait_overlay_enabled() {
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
    // Also install under telemetry-only for CADENCE MEASUREMENT: the present detour records the present-
    // cadence + GX semaphores read-only (the flow-modifying composite is separately gated off when the
    // overlay is not a product feature this run). Lets a flow-faithful vanilla baseline capture the
    // render-bound fingerprint (bd present-cadence-gx-instrumentation-coupled-to-overlay-install-gate;
    // VANILLA-run2-forcedrive-WORKS-...cadence-decouple-insufficient).
    if portrait_overlay_enabled()
        || save_override_telemetry_only()
        || crate::experiments::measure_no_composite()
    {
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
