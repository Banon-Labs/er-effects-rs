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

/// This stage's next tick count -- FROZEN while a modal OS file dialog is up.
///
/// Every save-flow deadline derives from `SAVE_FLOW_STAGE_TICKS`, which the game task increments
/// once per frame at exactly one site. The game task runs CONCURRENTLY with the menu/Scaleform
/// pump, so a modal dialog that blocks the pump does not stop the tick: a user browsing folders for
/// twenty seconds would spend ~1200 ticks and watch the destination-browser bound (180), the
/// confirm-box build bound (180) and eventually the commit watchdog (900) all expire underneath
/// them -- pick a file, nothing happens. Freezing this one READ freezes all of them.
///
/// The COUNTER is frozen, not the handlers. An early `return` from `save_flow_tick` would also
/// suspend the event-driven work that must keep running while a dialog is open (a box decision
/// arriving, the writer-idle teardown interlock, the IDLE-tick deferred-teardown sweep); a frozen
/// `ticks` value suspends only the deadlines. That is why this is a frozen read and not a skipped
/// tick.
///
/// Freezing is NECESSARY BUT NOT SUFFICIENT: stage 3's "abandoned" branch has no tick bound at all,
/// so it needs the separate liveness term in [`dest_browse_verdict`].
fn save_flow_next_stage_ticks(dialog_open: bool, counter: &AtomicUsize) -> usize {
    if dialog_open {
        SAVE_PICKER_OS_TICKS_FROZEN.fetch_add(1, Ordering::SeqCst);
        return counter.load(Ordering::SeqCst);
    }
    counter.fetch_add(1, Ordering::SeqCst) + 1
}

/// What stage 3 does with this tick. `WaitForUser` is every "do nothing this frame" case --
/// waiting on the user's choice, on the browser to appear, or on its teardown -- because the action
/// is identical in all three and only the terminal verdicts differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DestBrowseAction {
    /// A destination is committed and the browser is gone: close the menus and fire.
    CloseAndCommit,
    /// A committed destination whose browser will not tear down. Nothing is armed; abort.
    TeardownTimeout,
    /// Do nothing this frame.
    WaitForUser,
    /// A browser open that was staged for the menu pump and never appeared. Abort.
    OpenTimeout,
    /// No browser, no pending open, no commit, no confirm: the user backed out.
    Abandoned,
    /// An OS Save-As chose an existing file; the tick owes the Box3 overwrite confirm.
    EnterBox3,
}

/// Stage 3's decision, as a pure function of the latches it reads.
///
/// `os_dialog_open` is the term the OS surface adds, and it is not cosmetic. In OS mode, once the
/// menu pump has consumed `SAVE_DEST_OPEN_PICKER_PENDING` and blocked inside comdlg32, this tick
/// would see no commit pending, no `05_010` browser and no pending open -- and end the flow as
/// "abandoned" on the very next frame, a millisecond after the dialog appeared. No amount of tick
/// freezing prevents that, because that branch never looked at `ticks`. "A browser is live" simply
/// has two spellings now.
///
/// `confirm_pending` is checked ahead of every liveness term for the same reason: by the time an OS
/// Save-As has named an existing file its dialog is already gone, so a verdict that consulted
/// liveness first would read the flow as abandoned instead of owing a Box3.
#[allow(clippy::too_many_arguments)]
fn dest_browse_verdict(
    commit_pending: bool,
    picker_window_live: bool,
    dest_mode: bool,
    os_dialog_open: bool,
    confirm_pending: bool,
    open_pending: bool,
    ticks: usize,
) -> DestBrowseAction {
    // A confirmed destination outranks every other latch: the browser is on its way out and only
    // its teardown is being waited on.
    if commit_pending {
        if !picker_window_live {
            return DestBrowseAction::CloseAndCommit;
        }
        if ticks >= SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS {
            return DestBrowseAction::TeardownTimeout;
        }
        return DestBrowseAction::WaitForUser;
    }
    if confirm_pending {
        return DestBrowseAction::EnterBox3;
    }
    if dest_mode || os_dialog_open {
        // A browser owns the screen; the user's decision has no timeout.
        return DestBrowseAction::WaitForUser;
    }
    if open_pending {
        // Staged for the menu pump but not open yet.
        if ticks >= SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS {
            return DestBrowseAction::OpenTimeout;
        }
        return DestBrowseAction::WaitForUser;
    }
    DestBrowseAction::Abandoned
}

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
        // DEFERRED TEARDOWN SWEEP. A commit window is never taken out from under an executing
        // writer -- `save_dest_verify_and_disarm` refuses -- so a flow that returned to IDLE while
        // the SL worker was still inside a save-job body leaves the window behind on purpose. It
        // has to be closed the moment the writer finishes: an armed redirect that outlives its
        // commit would divert a LATER save of the loaded container to a destination nobody chose.
        if save_dest_commit_window_armed() && er_save_suppress::save_job_writer_idle() {
            let _ = save_dest_verify_and_disarm("deferred teardown after the writer finished");
            save_dest_reset("deferred teardown after the writer finished");
        }
        return;
    }
    let ticks = save_flow_next_stage_ticks(
        SAVE_PICKER_OS_DIALOG_OPEN.load(Ordering::SeqCst) != 0,
        &SAVE_FLOW_STAGE_TICKS,
    );
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
            // Declining the overwrite drops only the target. In-game the browser window was never
            // closed, so the user is simply back in it and nothing else is needed. In OS mode the
            // dialog is gone by the time Box3 is answered, so "back to the picker" means RE-OPEN
            // it -- which is exactly what Box2-No already does to open it in the first place, and
            // the menu pump's existing consumer now routes through `open_picker_for_intent`.
            save_dest_clear_target("box3 declined");
            if os_native_picker_active() {
                SAVE_DEST_OPEN_PICKER_PENDING.store(1, Ordering::SeqCst);
            }
            save_flow_enter_stage(
                SAVE_FLOW_STAGE_DEST_BROWSE,
                "box3 No -> back to the destination picker",
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
    let verdict = dest_browse_verdict(
        SAVE_DEST_COMMIT_PENDING.load(Ordering::SeqCst) != 0,
        SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0,
        SAVE_PICKER_DEST_MODE.load(Ordering::SeqCst) != 0,
        SAVE_PICKER_OS_DIALOG_OPEN.load(Ordering::SeqCst) != 0,
        SAVE_DEST_CONFIRM_PENDING.load(Ordering::SeqCst) != 0,
        SAVE_DEST_OPEN_PICKER_PENDING.load(Ordering::SeqCst) != 0,
        ticks,
    );
    match verdict {
        DestBrowseAction::WaitForUser => {}
        DestBrowseAction::CloseAndCommit => {
            // Gone: its close already restored the user's ProfileSummary rows and re-showed the
            // System windows, which is the state the close-all sequence expects. (In OS mode there
            // was never a picker window, so this is true on the first tick after the pick.)
            SAVE_DEST_COMMIT_PENDING.store(0, Ordering::SeqCst);
            unsafe { save_flow_close_menus_from_tick("dest_commit", true) };
        }
        DestBrowseAction::TeardownTimeout => {
            // The browser will not go away. Nothing has been armed or fired yet, so abort rather
            // than close the root menus out from under a live window.
            SAVE_DEST_COMMIT_PENDING.store(0, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: destination browser did not tear down after {ticks} ticks -- abandoning the commit, the user's save did NOT happen"
            ));
            unsafe { save_flow_close_menus_from_tick("dest_teardown_timeout", false) };
        }
        DestBrowseAction::EnterBox3 => {
            // OS Save-As named an existing file. The TICK performs the transition so the menu
            // thread never becomes a second writer of `SAVE_FLOW_STAGE`.
            SAVE_DEST_CONFIRM_PENDING.store(0, Ordering::SeqCst);
            // Box3 is hosted by the System dialog here: in OS mode there is no picker window job
            // occupying that queue, so it is the right owner.
            save_flow_box_set_host_dialog(0);
            SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_OVERWRITE_FILE, Ordering::SeqCst);
            save_flow_enter_stage(
                SAVE_FLOW_STAGE_BOX3_WAIT,
                "os save-as chose an existing file -> overwrite confirm",
            );
        }
        DestBrowseAction::OpenTimeout => {
            SAVE_DEST_OPEN_PICKER_PENDING.store(0, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: destination browser never opened after {ticks} ticks -- ending the flow, the user's save did NOT happen"
            ));
            unsafe { save_flow_close_menus_from_tick("dest_picker_open_timeout", false) };
        }
        DestBrowseAction::Abandoned => {
            // No browser, no pending open, no commit, no confirm: the user backed out. In OS mode
            // this is exactly what dropping the dialog latch with nothing chosen looks like -- and
            // reaching it at all took the menu pump learning to stop re-arming the open request it
            // had already spent (bd `er-effects-rs-rsxi`).
            SAVE_DEST_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
            save_dest_clear_target("destination browser abandoned");
            if os_native_picker_active() {
                // OS SURFACE: the destination browser was comdlg32, so NOTHING of ours was ever
                // opened over the menus -- the System>Quit stack is bit-for-bit the stack the user
                // pressed Save Game on, and there is nothing to unwind. Closing the menus here
                // would take them out to the world, one level FURTHER than the Back they pressed.
                // End the flow instead: control returns to the System>Quit rows, and returning to
                // IDLE is what re-opens the row-press guard so Save Game can be pressed again.
                // This is the same place the OS LOAD surface's cancel already leaves the user.
                SAVE_FLOW_ABORT_COUNT.fetch_add(1, Ordering::SeqCst);
                save_flow_box_clear();
                save_dest_reset("os destination picker dismissed");
                append_autoload_debug(format_args!(
                    "save-flow: OS save-as dismissed after {ticks} ticks -- ending the flow at the System>Quit menu with NOTHING written, staged or loaded"
                ));
                save_flow_enter_stage(
                    SAVE_FLOW_STAGE_IDLE,
                    "os destination picker dismissed; back at System>Quit",
                );
                return;
            }
            append_autoload_debug(format_args!(
                "save-flow: destination browser closed without choosing after {ticks} ticks -- returning to the world with nothing written"
            ));
            unsafe { save_flow_close_menus_from_tick("dest_abandoned", false) };
        }
    }
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

/// What a Save Game commit is about to write, decided before anything is written.
enum SaveFlowCommitPlan {
    /// The loaded save is the target: Box2 answered "Yes", or a browsed pick that the filesystem
    /// says IS the loaded save. The native writer rewrites it in place and nothing is redirected.
    LiveOverwrite { live: PathBuf, reason: &'static str },
    /// A browsed destination PROVEN to be a different file from the loaded save.
    Redirect { live: PathBuf, target: PathBuf },
    /// No destination was chosen and no loaded-save path could be resolved, so there is no file
    /// to name or protect. The request still fires; nothing is armed.
    Unnamed,
}

/// Decide what this commit will write, and refuse rather than guess.
///
/// PERFORMS NO WRITES. Everything that can turn this commit down happens here, while the
/// destination is still exactly as the user left it. Two refusals are new, and both exist because
/// the alternative was a save written over the wrong file:
///
/// * **Identity that cannot be established.** `Unknown` from the handle probe means the commit
///   cannot tell whether the destination is the loaded save. Guessing "different" seeds and
///   redirects the loaded save onto itself, and the leak check then restores the pre-fire
///   snapshot over the save that just succeeded. A refused save is recoverable; that is not.
/// * **No writer-completion observer.** Without the SL save-job-body observer, nothing can say
///   when the native writer has finished, so the redirect window would have to close on a tick
///   count -- and the in-place writer opens the container once per dirty block, so a window that
///   closes early patches the remaining blocks into the loaded save. A redirected commit is not
///   safe to fire without that signal. (The loaded-save overwrite has no window to leak and is
///   unaffected.)
fn save_flow_resolve_commit_plan() -> Result<SaveFlowCommitPlan, String> {
    let Some(target) = save_dest_target() else {
        // Box2 answered "Yes": no destination was ever chosen because the destination IS the
        // loaded save.
        return Ok(match save_dest_live_save_path() {
            Some(live) => SaveFlowCommitPlan::LiveOverwrite {
                live,
                reason: "box2 overwrite-the-loaded-save",
            },
            None => SaveFlowCommitPlan::Unnamed,
        });
    };
    let Some(live) = save_dest_live_save_path() else {
        SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
        return Err(format!(
            "destination '{}' was chosen but the loaded save path is unavailable, so the commit cannot tell whether they are the same file",
            target.display()
        ));
    };
    match save_dest_commit_identity(&target, &live) {
        SaveDestIdentity::SameFile => {
            let spelled_differently = save_dest_normalize_path_of(&target)
                != save_dest_normalize_path_of(&live)
                || save_dest_normalize_path_of(&target).is_none();
            if spelled_differently {
                SAVE_DEST_SELF_REDIRECT_BLOCKED.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-dest: the browsed destination '{}' and the loaded save '{}' are spelled differently but are THE SAME FILE (handle identity) -- taking the overwrite path. Redirecting a file onto itself makes the save land and then look like a leak, and the safety net would write the pre-fire snapshot over it",
                    target.display(),
                    live.display()
                ));
            }
            Ok(SaveFlowCommitPlan::LiveOverwrite {
                live,
                reason: "browsed destination is the loaded save",
            })
        }
        SaveDestIdentity::Distinct => {
            if !er_save_suppress::save_job_observer_installed() {
                SAVE_DEST_NO_WRITER_OBSERVER_ABORT.fetch_add(1, Ordering::SeqCst);
                SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
                return Err(format!(
                    "destination '{}' needs the write-open redirect, but the SL save-job-body observer is not installed, so nothing can tell when the native writer has finished. The redirect window would have to close on a tick count, and the in-place writer opens the container once per dirty block -- closing early would patch the rest into the loaded save",
                    target.display()
                ));
            }
            Ok(SaveFlowCommitPlan::Redirect { live, target })
        }
        SaveDestIdentity::Unknown => {
            SAVE_DEST_IDENTITY_UNKNOWN_ABORT.fetch_add(1, Ordering::SeqCst);
            SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
            Err(format!(
                "the destination '{}' could not be PROVEN either identical to, or distinct from, the loaded save '{}' (the filesystem gave no usable identity for one of them)",
                target.display(),
                live.display()
            ))
        }
    }
}

/// Stand-in status for "the freshness handshake said an outcome was waiting and it was gone by
/// the time it was taken". Structurally unreachable -- both run on the game thread within a few
/// statements -- and deliberately NOT zero, so it can never be mistaken for a success.
const SAVE_FLOW_STATUS_LOST: u32 = u32::MAX;

/// Normalized text form of a path, for the "were these two spelled differently?" report only.
fn save_dest_normalize_path_of(path: &std::path::Path) -> Option<String> {
    path.to_str().and_then(save_dest_normalize_path)
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
        // DESTINATION COMMIT (save-game-flow WP3). DECIDE FIRST, WRITE LAST (2026-07-29): the plan
        // below performs no I/O on the destination, so every refusal -- an unprovable identity, a
        // missing writer observer, a token already pending -- happens while the user's chosen file
        // is still untouched. The seed, which is the first byte this flow writes anywhere, is only
        // laid down afterwards, once nothing is left that can abort. The old order seeded ~29 MB
        // over the destination and could then still bail out and report that the save did not
        // happen, having already overwritten the file it was reporting about.
        let plan = match save_flow_resolve_commit_plan() {
            Ok(plan) => plan,
            Err(why) => {
                append_autoload_debug(format_args!(
                    "save-flow: FIRE ABORT -- {why}. NOT firing; nothing has been written and the user's save did NOT happen"
                ));
                save_dest_reset("commit plan refused");
                save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "commit plan refused");
                return;
            }
        };
        // Degraded fail-open (prologue mismatch / install failure): suppression never armed, so
        // every native save already writes normally and there is no token to arm or watch.
        let degraded = !er_save_suppress::is_armed();
        if !degraded && !er_save_suppress::arm_one_save_bypass() {
            // Refusal here means a token is already pending -- some earlier commit's
            // watchdog has not run yet. Abort rather than fire into an ambiguous token.
            append_autoload_debug(format_args!(
                "save-flow: FIRE ABORT -- arm_one_save_bypass refused (token already pending); nothing has been written and the user's save did NOT happen"
            ));
            save_dest_reset("bypass arm refused");
            save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "bypass arm refused");
            return;
        }
        // THE FIRST WRITE. Naming the file is not optional either: a rewrite of the live
        // `ER0000.sl2` that nothing in the log claims is indistinguishable after the fact from a
        // suppression leak or a staging copy.
        match &plan {
            SaveFlowCommitPlan::LiveOverwrite { live, reason } => {
                save_dest_arm_live_overwrite(live, reason);
            }
            SaveFlowCommitPlan::Redirect { live, target } => {
                if !save_dest_arm_redirect(live, target) {
                    SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
                    if !degraded {
                        // The token was armed a moment ago and nothing will consume it now.
                        // Leaving it pending would let the NEXT native save through for real.
                        let _ = er_save_suppress::expire_bypass_if_pending();
                    }
                    append_autoload_debug(format_args!(
                        "save-flow: FIRE ABORT -- could not arm the destination redirect for '{}'; NOT firing, the user's save did NOT happen",
                        target.display()
                    ));
                    save_dest_reset("redirect arm failed");
                    save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "destination arm failed");
                    return;
                }
            }
            SaveFlowCommitPlan::Unnamed => {}
        }
        if degraded {
            append_autoload_debug(format_args!(
                "save-flow: suppression NOT armed (oracle_save_suppress_armed=0) -- degraded fail-open: firing the forced native save request without a bypass token. Completion for this commit comes from the SL writer's own job-body signal, NOT from token consumption, which can never move on this path"
            ));
        }
        // Sample the request flags BEFORE the fire. This is the whole basis of the scoped
        // retraction below: a flag already set here belongs to the game, a flag that goes
        // 0 -> 1 across our own call is ours. An unreadable GameMan disqualifies that flag
        // from retraction rather than defaulting it to "was clear".
        let flag_before = |offset: usize| -> usize {
            if gm < HEAP_LO {
                return SAVE_FLOW_FLAG_UNREAD;
            }
            unsafe { safe_read_u8(gm + offset) }.map_or(SAVE_FLOW_FLAG_UNREAD, usize::from)
        };
        SAVE_FLOW_B72_BEFORE_FIRE
            .store(flag_before(GAME_MAN_ARM_FLAG_B72_OFFSET), Ordering::SeqCst);
        SAVE_FLOW_B73_BEFORE_FIRE.store(
            flag_before(GAME_MAN_FLAG_B73_PROBE_OFFSET),
            Ordering::SeqCst,
        );
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
        // Snapshot the consumed-token counter so stage 8 can tell a save that actually reached
        // the writer from a fire that silently went nowhere -- and, alongside it, the
        // native-side attribution counters so a failure can name WHICH link broke instead of
        // only reporting that the enqueue never arrived (see `save_flow_fire_failure_reason`).
        SAVE_FLOW_BYPASS_ALLOWED_AT_FIRE.store(
            usize::try_from(er_save_suppress::bypass_allowed_total()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        SAVE_FLOW_DISPATCH_CALLS_AT_FIRE.store(
            usize::try_from(er_save_suppress::dispatch_calls()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        SAVE_FLOW_DISPATCH_DECLINES_AT_FIRE.store(
            usize::try_from(er_save_suppress::dispatch_declines()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        SAVE_FLOW_SERIALIZE_CALLS_AT_FIRE.store(
            usize::try_from(er_save_suppress::serialize_calls()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        SAVE_FLOW_SERIALIZE_FAILURES_AT_FIRE.store(
            usize::try_from(er_save_suppress::serialize_failures()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        SAVE_FLOW_SUBMITS_SWALLOWED_AT_FIRE.store(
            usize::try_from(er_save_suppress::submits_swallowed()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        // The SL worker's own start and completion counters, plus a cleared start-tick, so stage 8
        // can say WHEN the native write began and separate "the write is slow" from "we were slow
        // to notice it finished". The COMPLETION baseline is also the teardown interlock: the
        // redirect window may only be dropped once a job body has returned past this value, or
        // once it is known no body can start.
        SAVE_FLOW_SAVE_JOB_STARTS_AT_FIRE.store(
            usize::try_from(er_save_suppress::save_job_starts()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        SAVE_FLOW_SAVE_JOB_COMPLETIONS_AT_FIRE.store(
            usize::try_from(er_save_suppress::save_job_completions()).unwrap_or(usize::MAX),
            Ordering::SeqCst,
        );
        SAVE_FLOW_DEGRADED_FIRE.store(usize::from(degraded), Ordering::SeqCst);
        SAVE_FLOW_COMMIT_JOB_START_TICK.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow: FIRED forced save request (throttle skipped) after {ticks} gate ticks; readback b72={b72} b73={b73} degraded={degraded}"
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

/// Name the link that broke when a fired Save Game commit produced no write.
///
/// This exists because "no save enqueue arrived" is the SAME observation for every failure
/// along the chain, and telling them apart used to take a run each. The native chain a fired
/// request has to walk (1.16.2 decompile) is:
///
/// ```text
///   RequestSave/SaveRequest_Profile   set GameMan+0xb72 / +0xb73
///     -> FUN_140afb880                 the per-frame dispatcher in MoveMapStep step 18
///          gate: saveState == 0, ShouldSave()/b73 getter, !BOOL_143d856a0
///     -> FUN_14067b940 / b750 / b570   the lane
///     -> FUN_14067dc00                 the character serializer (character lanes only)
///     -> FUN_140e6ef60 / FUN_140e6ec70 the submit builder
///     -> FUN_140e6fb50                 the SL enqueue -- where the one-shot bypass lives
/// ```
///
/// Only the last link is ours. A lane that returns 0 touches NOTHING (the request flags stay
/// set, `saveState` stays 0), so the dispatcher re-enters it every frame and the failure is
/// invisible from the enqueue: exactly the shape that made a real run read as "the fire went
/// nowhere" with no way to tell whether anything downstream had even been attempted.
///
/// Deltas, not totals: the counters are process-lifetime and a session has already swallowed
/// boot saves before the row is ever pressed.
///
/// The lane's own refusal splits three ways, and `serializer_calls` is what splits it. Both
/// character lanes allocate their MainHeap buffers and null-check them BEFORE calling
/// `FUN_14067dc00` (`FUN_14067b940`: `0x280000` then `0x60000`, both checked;
/// `FUN_14067b750`: `0x280000`), so:
///
/// * declines up, `serializer_calls` FLAT -> the lane never reached the serializer: an
///   allocation returned null, or a pre-allocation gate refused.
/// * `serializer_failures` up -> allocations fine, serializer entered and refused; the
///   step decoder names where.
/// * declines up, `serializer_calls` up, `serializer_failures` FLAT -> allocations fine,
///   serialization fine, and the submit builder is what refused.
///
/// Each arm says which of those it was in words, so the verdict does not have to be
/// reconstructed from two counters after the fact.
fn save_flow_fire_failure_reason() -> String {
    if er_save_suppress::dispatch_observers_installed() == 0 {
        return "attribution unavailable: the save-dispatch observers are not installed on this \
                build, so the native chain between the request flags and the enqueue cannot be \
                read"
            .to_owned();
    }
    let since = |now: u64, at_fire: &core::sync::atomic::AtomicUsize| -> u64 {
        now.saturating_sub(at_fire.load(Ordering::SeqCst) as u64)
    };
    let dispatch = since(
        er_save_suppress::dispatch_calls(),
        &SAVE_FLOW_DISPATCH_CALLS_AT_FIRE,
    );
    let declines = since(
        er_save_suppress::dispatch_declines(),
        &SAVE_FLOW_DISPATCH_DECLINES_AT_FIRE,
    );
    let serialize_calls = since(
        er_save_suppress::serialize_calls(),
        &SAVE_FLOW_SERIALIZE_CALLS_AT_FIRE,
    );
    let serialize_fails = since(
        er_save_suppress::serialize_failures(),
        &SAVE_FLOW_SERIALIZE_FAILURES_AT_FIRE,
    );
    let swallowed = since(
        er_save_suppress::submits_swallowed(),
        &SAVE_FLOW_SUBMITS_SWALLOWED_AT_FIRE,
    );
    let facts = format!(
        "since the fire: dispatch_entries={dispatch} declines={declines} \
         declines_with_bypass={} serializer_calls={serialize_calls} \
         serializer_failures={serialize_fails} serializer_last_fail_bytes={} \
         serializer_last_fail_step={} submits_swallowed={swallowed} last_lane={}",
        er_save_suppress::dispatch_declines_with_bypass(),
        er_save_suppress::serialize_last_fail_bytes(),
        er_save_suppress::serialize_last_fail_step(),
        er_save_suppress::dispatch_last_lane()
    );
    // Owned, because the serializer arm names the failing sub-serializer inline.
    let verdict: String = if swallowed > 0 {
        "WE SWALLOWED IT: a submit was built after the fire and this DLL's suppressor ate it \
         instead of letting the bypass token through -- the fault is ours, in the bypass \
         token/enqueue handshake"
            .to_owned()
    } else if serialize_fails > 0 {
        // The allocations are NOT in question on this path: the lane null-checks its
        // MainHeap buffers before it can reach FUN_14067dc00 at all, so entering the
        // serializer is proof they succeeded.
        format!(
            "THE CHARACTER SERIALIZER REFUSED: the lane's MainHeap buffers allocated fine and \
             FUN_14067dc00 was entered, but it returned 0, so the lane skipped the submit \
             builder entirely. Nothing downstream (submit, enqueue, bypass) was ever \
             reachable. It refused at: {}",
            er_save_suppress::serialize_fail_step_detail(
                er_save_suppress::serialize_last_fail_bytes()
            )
        )
    } else if declines > 0 && serialize_calls == 0 {
        "THE LANE BAILED BEFORE THE SERIALIZER: a save lane was entered and returned 0 without \
         ever calling FUN_14067dc00, so the request flags stay latched and no submit is built. \
         Every exit ahead of the serializer is pre-serialization: the 0x280000 (and, on the \
         combined lane, 0x60000) MainHeap allocations returned null, or one of the gates in \
         front of them turned the lane away -- CanShowSaveMenu() true, GameMan.saveState != 0, \
         or a slot index >= 10"
            .to_owned()
    } else if declines > 0 {
        "THE SUBMIT BUILDER REFUSED: the lane allocated its buffers, FUN_14067dc00 SUCCEEDED, \
         and the lane still returned 0 -- so the refusal is FUN_140e6ef60 / FUN_140e6ec70, \
         between a good serialization and the SL enqueue. Serialization and allocation are \
         both ruled out"
            .to_owned()
    } else if dispatch > 0 {
        "THE DISPATCHER RAN AND SUCCEEDED BUT NO ENQUEUE ARRIVED: a lane returned non-zero \
         yet FUN_140e6fb50 was never reached, which contradicts the decompiled chain -- treat \
         the hook set as suspect"
            .to_owned()
    } else {
        "THE DISPATCHER NEVER RAN: no save lane was entered at all after the request flags \
         were set, so FUN_140afb880's own gate refused (saveState != 0, ShouldSave()/b73 \
         getter false, or the global save-suppress byte 0x143d856a0 set)"
            .to_owned()
    };
    format!("{verdict} [{facts}]")
}

/// Take back the save-request flags OUR fire set, once that fire has provably gone nowhere.
///
/// # Why this is not optional
///
/// A save lane that refuses touches nothing: `GameMan+0xb72`/`+0xb73` stay set, `saveState`
/// stays 0, so `FUN_140afb880` re-enters the refusing lane on the very next frame and does
/// it again forever. Every entry runs the full character serializer into a 0x280000 (2.6 MB)
/// buffer and throws the result away. Measured on a stuck run: 27,824 declines with ZERO
/// serializer failures over 854 s -- about 33 complete character serializations per second,
/// ~73 GB produced and discarded in fourteen minutes, on the game thread, for the rest of
/// the session. That is not a stalled UI row; it is a permanent CPU burn in a build someone
/// is playing.
///
/// # Why it is safe, and how it stays scoped
///
/// Four conditions, all of which must hold. Any one failing leaves the flags alone and
/// counts `SAVE_FLOW_RETRACT_DECLINED`:
///
/// 1. **Suppression is armed.** With `er_save_suppress` armed, NO save reaches disk except
///    a bypassed one, so a native request we drop would have been swallowed anyway -- the
///    retraction cannot lose a byte that would otherwise have been written. On the degraded
///    fail-open path (suppression not armed) native saves are real, so we never retract.
/// 2. **No return-title sequence is in flight** (`GameMan+0xbc4 == 0`). The quit chain
///    advances `bc4` 1 -> 2 -> 3 *through* a dispatched save; retracting a request during
///    that window is exactly how System->Quit would hang. `bc4 != 0` means hands off.
/// 3. **The flag was clear before our own fire.** Sampled into
///    `SAVE_FLOW_B72_BEFORE_FIRE`/`_B73_` immediately before the forced pair. A flag the
///    game had already set is not ours and is left set; an unreadable pre-sample is treated
///    as "not ours" too.
/// 4. **No submit was built.** The caller only reaches here after the enqueue grace window
///    expired with the one-shot token unconsumed -- i.e. the dispatcher ran, refused, and
///    nothing downstream ever happened.
///
/// The residual case this cannot separate is a request the game raised *inside* our own
/// fire-to-bailout window: it and ours are one shared byte, and no read can tell them
/// apart. Condition 1 is what makes that acceptable -- that request could not have written
/// anything either -- and the game re-raises its own autosave requests from its own
/// triggers on the next event.
fn save_flow_retract_stuck_request(reason: &str) {
    const HEAP_LO: usize = 0x10000;
    const FLAG_SET: usize = 1;
    const RETURN_TITLE_IDLE: u8 = 0;

    let decline = |why: &str| {
        SAVE_FLOW_RETRACT_DECLINED.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow: NOT retracting the latched save request after '{reason}' -- {why}. The \
             native dispatcher will keep re-entering the refusing lane every frame (each entry \
             is a full 0x280000 character serialization) until something else clears the flags"
        ));
    };
    if !er_save_suppress::is_armed() {
        decline(
            "suppression is not armed, so native saves are real and a dropped request would be \
             a dropped save",
        );
        return;
    }
    let Ok(base) = game_module_base() else {
        decline("the game module base is unavailable");
        return;
    };
    let Some(gm) = (unsafe { safe_read_usize(base + er_game_base::rva::GAME_MAN_SINGLETON_RVA) })
    else {
        decline("GameMan is unreadable");
        return;
    };
    if gm < HEAP_LO {
        decline("GameMan is not heap-like");
        return;
    }
    let quit_phase = unsafe { safe_read_u8(gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) };
    match quit_phase {
        Some(RETURN_TITLE_IDLE) => {}
        Some(phase) => {
            decline(&format!(
                "a return-title sequence is in flight (GameMan+0xbc4={phase}); its 1 -> 2 -> 3 \
                 advance runs THROUGH a dispatched save, so retracting here is how the quit hangs"
            ));
            return;
        }
        None => {
            decline("GameMan+0xbc4 is unreadable, so a quit sequence cannot be ruled out");
            return;
        }
    }
    // Ours only: clear a flag exactly when it was 0 before our fire and is 1 now.
    let ours = |offset: usize, before: &core::sync::atomic::AtomicUsize| -> bool {
        if before.load(Ordering::SeqCst) != 0 {
            return false;
        }
        unsafe { safe_read_u8(gm + offset) }.is_some_and(|now| usize::from(now) == FLAG_SET)
    };
    let take_b72 = ours(GAME_MAN_ARM_FLAG_B72_OFFSET, &SAVE_FLOW_B72_BEFORE_FIRE);
    let take_b73 = ours(GAME_MAN_FLAG_B73_PROBE_OFFSET, &SAVE_FLOW_B73_BEFORE_FIRE);
    if !take_b72 && !take_b73 {
        decline(
            "neither flag went 0 -> 1 across our own fire, so whatever is latched now is not \
             ours to take back",
        );
        return;
    }
    let (cleared_b72, cleared_b73) =
        unsafe { system_quit_save_request_retract(take_b72, take_b73) };
    let cleared = usize::from(cleared_b72) + usize::from(cleared_b73);
    if cleared == 0 {
        decline("both native retractions failed their byte verification");
        return;
    }
    SAVE_FLOW_REQUEST_RETRACTIONS.fetch_add(cleared, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-flow: RETRACTED the latched save request after '{reason}' (b72={cleared_b72} \
         b73={cleared_b73}) through the game's own FUN_140678740/FUN_140678710 -- the fire went \
         nowhere and the flags were ours, so the per-frame re-serialization spin stops here"
    ));
}

/// Stage 8 COMMIT_WAIT: the forced request is in flight through the native pump with the
/// bypass token armed.
///
/// COMPLETION IS AN EVENT, NOT A TIMEOUT (2026-07-28). This used to wait exclusively on the
/// status poll's terminal answer, and a measured commit that wrote and verified the user's
/// save reported `bypass_final_status = null` and `commit_complete = 0` twenty-one seconds
/// later, ending on the watchdog: the Save Game row is gated on the flow being IDLE, so a
/// successful save froze the row for the whole watchdog AND under-reported itself as a
/// non-completion. The poll cannot be the primary signal here -- see the write-completion
/// section in `er-save-suppress` for why (its "terminal" needs the worker to have DEQUEUED
/// the job, and its only two consumers are a MenuJob this flow deliberately closes before
/// firing and a `MoveMapStep` branch gated on the game still thinking a save is in flight).
///
/// The ordering below is deliberate and load-bearing:
///
///   1. adopt the SL worker's job-body completion, if one has arrived -- the event that
///      says the write finished, produced by the game on its own writer thread;
///   2. hold everything while a save-job body is EXECUTING, because tearing the redirect
///      window down mid-body sends the writer's remaining per-block opens to the loaded save;
///   3. consume any terminal status (from step 1, from a native poll consumer, or from a
///      submit the native enqueue refused) and issue the ONE verdict, file check included;
///   4. only then consider the enqueue-grace bailout, and only when the one-shot token can
///      be revoked, which is what proves nothing is in flight;
///   5. the watchdog last, as a backstop that is counted as a DEGRADED outcome.
///
/// The degraded fail-open path has no token at all and is handled separately -- see
/// [`save_flow_degraded_commit_wait_tick`].
fn save_flow_commit_wait_tick(ticks: usize) {
    let completions_at_fire = SAVE_FLOW_SAVE_JOB_COMPLETIONS_AT_FIRE.load(Ordering::SeqCst) as u64;
    // Timestamp the moment the SL worker picked the job up. Cheap, and it is the number
    // that says whether a long commit was a slow WRITE or a slow OBSERVATION.
    if SAVE_FLOW_COMMIT_JOB_START_TICK.load(Ordering::SeqCst) == 0
        && usize::try_from(er_save_suppress::save_job_starts()).unwrap_or(usize::MAX)
            > SAVE_FLOW_SAVE_JOB_STARTS_AT_FIRE.load(Ordering::SeqCst)
    {
        SAVE_FLOW_COMMIT_JOB_START_TICK.store(ticks, Ordering::SeqCst);
    }
    if SAVE_FLOW_DEGRADED_FIRE.load(Ordering::SeqCst) != 0 {
        save_flow_degraded_commit_wait_tick(ticks, completions_at_fire);
        return;
    }
    // POSITIVE EVIDENCE ONLY: this latches a status when, and only when, a save job that
    // started after our own submit has RETURNED from its body, and it reports whatever
    // result the game itself recorded for it. It cannot fire on "no failure seen yet".
    let _ = er_save_suppress::adopt_completed_save_job_as_final_status();
    // NOTHING BELOW MAY RUN WHILE THE WRITER IS INSIDE A JOB BODY. Every exit from this stage
    // disarms the commit window, and the native in-place writer opens the save container ONCE
    // PER DIRTY BLOCK: a window closed between block k and k+1 sends blocks k+1..N to the
    // loaded save with `OPEN_ALWAYS` (no truncate), after the leak check has already run, so
    // nothing detects or undoes it. A latched terminal status keeps its freshness flag and is
    // taken on a later tick instead.
    if !save_dest_teardown_allowed(completions_at_fire, "commit wait") {
        return;
    }
    if er_save_suppress::bypass_final_status_fresh() {
        // THE FILE CHECK RUNS FIRST AND HAS THE LAST WORD (2026-07-28). `status` is the game's SL
        // job result -- its opinion of its own bookkeeping, which run 4 reported as 0 (success) for
        // a commit that produced a sparse, headerless fragment. Announcing that status and then
        // contradicting it a line later made the log say "COMMIT COMPLETE" above "the user's save
        // did NOT land". Score the bytes, then emit ONE verdict that folds both in.
        //
        // The status is PEEKED, not taken, until the file check has actually run: the disarm has
        // its own hard interlock against an executing writer, and a status consumed and then
        // dropped on that deferral would be an outcome nobody ever reports.
        let window_was_armed = save_dest_commit_window_armed();
        let verdict = save_dest_verify_and_disarm("commit terminal status");
        if window_was_armed && verdict.is_none() {
            return;
        }
        let status = er_save_suppress::take_bypass_final_status().unwrap_or(SAVE_FLOW_STATUS_LOST);
        let status_ok = status == 0;
        // `None` = no commit window was armed (degraded paths). Nothing to contradict.
        let file_ok = verdict.as_ref().is_none_or(|verdict| verdict.ok);
        let file_state = match verdict.as_ref() {
            Some(verdict) if verdict.ok => "VERIFIED",
            Some(_) => "FAILED",
            None => "not armed",
        };
        let detail = verdict.as_ref().map_or_else(
            || "no commit window was armed, so no file could be checked".to_owned(),
            |verdict| verdict.summary.clone(),
        );
        // Name the observation that ended the commit and when the writer started, so a slow
        // commit can be attributed without another run: `write started tick N of M` is the
        // native write's own duration, and the remainder is detection latency.
        let source = er_save_suppress::bypass_final_status_source();
        let source_label = er_save_suppress::bypass_final_status_source_label(source);
        let started = SAVE_FLOW_COMMIT_JOB_START_TICK.load(Ordering::SeqCst);
        let timing = if started == 0 {
            "the SL worker was never seen to start writing".to_owned()
        } else {
            format!("the SL worker started writing on commit tick {started} of {ticks}")
        };
        if status_ok && file_ok {
            SAVE_FLOW_COMMIT_COMPLETE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: COMMIT COMPLETE after {ticks} commit ticks -- the bypassed save reported terminal status 0 (observed via {source_label}) AND the file VERIFIED on disk; {timing}: {detail}"
            ));
        } else {
            if status_ok {
                SAVE_FLOW_COMMIT_VERIFY_FAIL_COUNT.fetch_add(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "save-flow: COMMIT FAILED after {ticks} commit ticks -- terminal status {status} (0=success, observed via {source_label}), file check {file_state}; {timing}: {detail}. The user's save did NOT land"
            ));
        }
        save_dest_reset("commit terminal status");
        save_flow_enter_stage(
            SAVE_FLOW_STAGE_IDLE,
            if status_ok && file_ok {
                "commit verified"
            } else {
                "commit FAILED -- nothing usable was written"
            },
        );
        return;
    }
    // DEAD-FIRE BAILOUT (user-reported 2026-07-28): the Save Game row is gated on the flow being
    // IDLE, so a stage 8 that can never complete freezes the row for the whole watchdog -- ~15-30 s
    // of "the menus don't work any more" after a single save. Distinguish the two cases by whether
    // the one-shot token was ever CONSUMED:
    //   * consumed  -> a real write is in flight; keep the full watchdog and protect it.
    //   * unconsumed -> the fire set the native request flags but no save enqueue ever reached the
    //     suppressor, so nothing is in flight, the flow is already dead, and holding the row
    //     hostage protects nothing. Report the failure and free the UI immediately.
    let allowed_since_fire = || {
        usize::try_from(er_save_suppress::bypass_allowed_total()).unwrap_or(usize::MAX)
            > SAVE_FLOW_BYPASS_ALLOWED_AT_FIRE.load(Ordering::SeqCst)
    };
    let consumed = allowed_since_fire();
    if !consumed && ticks >= SAVE_FLOW_ENQUEUE_GRACE_TICKS {
        // THE TOKEN IS THE INTERLOCK (2026-07-28). The read above and this call are not
        // atomic together: an enqueue can arrive between them and take the token, and this
        // branch would then retract the request flags and free the row while a genuine write
        // was starting -- the one outcome worse than waiting. Both sides take the token with
        // the same CAS, so exactly one can win, and only winning it proves nothing is or will
        // be in flight. A false here with the token now consumed means the enqueue won: keep
        // waiting for the write it started.
        let expired = er_save_suppress::expire_bypass_if_pending();
        if !expired && allowed_since_fire() {
            append_autoload_debug(format_args!(
                "save-flow: enqueue-grace bailout STOOD DOWN at {ticks} ticks -- a save enqueue took the one-shot token while the bailout was deciding, so a real write is now in flight; waiting for it instead of retracting it"
            ));
            return;
        }
        SAVE_FLOW_ENQUEUE_MISSING_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow: FIRE WENT NOWHERE -- no save enqueue reached the writer within {ticks} ticks (token_expired={expired}); the user's save did NOT happen. {}. Ending the flow now so the Save Game row is usable again instead of blocking for the full {SAVE_BYPASS_WATCHDOG_TICKS}-tick watchdog",
            save_flow_fire_failure_reason()
        ));
        let _ = save_dest_verify_and_disarm("fire went nowhere");
        save_dest_reset("fire went nowhere");
        // The flags our fire set are still latched and the lane refuses them every frame.
        // Freeing the UI without taking them back leaves a full character serialization
        // running ~33 times a second for the rest of the session.
        save_flow_retract_stuck_request("fire went nowhere");
        save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "fire went nowhere");
        return;
    }
    if ticks >= SAVE_BYPASS_WATCHDOG_TICKS {
        // A CONSUMED TOKEN WITH NO JOB YET IS A WRITE THAT HAS NOT HAPPENED. The enqueue was
        // forwarded for real, so the SL worker may still pick the job up; disarming the redirect
        // in front of it points the writer's per-block opens at the loaded save. Waiting is the
        // safe side, so the window is held past the watchdog -- bounded only so a permanently
        // stalled queue cannot disable the Save Game row for the rest of the session, and the
        // moment that bound is reached is a NAMED failure rather than a quiet timeout.
        if consumed
            && save_dest_commit_window_armed()
            && save_dest_writer_state(completions_at_fire) == SaveDestWriterState::NotStarted
        {
            if ticks == SAVE_BYPASS_WATCHDOG_TICKS {
                append_autoload_debug(format_args!(
                    "save-flow: commit watchdog reached at {ticks} ticks but the one-shot token WAS consumed and the SL worker has not started the job yet -- holding the destination redirect open for up to {SAVE_DEST_TEARDOWN_UNPROVEN_EXTRA_TICKS} more ticks, because disarming in front of a queued write sends it to the loaded save"
                ));
            }
            if ticks < SAVE_BYPASS_WATCHDOG_TICKS + SAVE_DEST_TEARDOWN_UNPROVEN_EXTRA_TICKS {
                return;
            }
            SAVE_DEST_DISARM_UNPROVEN.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: the destination redirect is being disarmed WITHOUT PROOF the writer ran -- the enqueue was forwarded {ticks} ticks ago and no save-job body ever started. If the SL queue delivers it later it will write the loaded save, and nothing here can stop that"
            ));
        }
        let expired = er_save_suppress::expire_bypass_if_pending();
        // A BACKSTOP REACHED IS A DEGRADED OUTCOME, AND IT IS COUNTED. Every other exit from
        // this stage knows what happened; this one is defined by never having found out, and
        // the file check below cannot repair that (it scores bytes, not the write's own
        // verdict). Counting it is what stops "we never observed the save" from looking like
        // silence in the telemetry.
        SAVE_FLOW_COMMIT_WATCHDOG_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow: COMMIT WATCHDOG after {ticks} ticks -- {}; the user's save did NOT happen. Write-completion observer installed={}, save jobs started/completed since boot={}/{}. {}",
            if expired {
                "one-shot bypass token was still pending and has been expired"
            } else {
                "token was consumed but no terminal status was observed"
            },
            er_save_suppress::save_job_observer_installed(),
            er_save_suppress::save_job_starts(),
            er_save_suppress::save_job_completions(),
            save_flow_fire_failure_reason()
        ));
        // A destination write may still have landed (the degraded fail-open path has no token to
        // watch at all), so score it before dropping the window rather than assuming failure.
        let _ = save_dest_verify_and_disarm("commit watchdog");
        save_dest_reset("commit watchdog");
        // Same spin, reached the slow way: the token was consumed but no terminal status
        // ever arrived, or the grace path did not fire. `save_flow_retract_stuck_request`
        // re-checks ownership itself, so a request that already retired is a no-op here.
        save_flow_retract_stuck_request("commit watchdog");
        save_flow_enter_stage(SAVE_FLOW_STAGE_IDLE, "commit watchdog");
    }
}

/// Stage 8 for a commit fired on the DEGRADED fail-open path: suppression never armed, so no
/// bypass token exists and the save the request produces is an ordinary native write.
///
/// This exists because the token-based stage 8 cannot describe this path at all. Its
/// completion test reads `bypass_allowed_total`, which only moves when a token is CONSUMED;
/// with no token armed it is frozen by construction, so the "was the fire dead?" predicate was
/// permanently true. Every degraded commit therefore ran to the 3 s enqueue-grace bailout,
/// disarmed the commit window and reported "the user's save did NOT happen" -- including the
/// ones where the native write had already succeeded. And `take_bypass_final_status` can never
/// return `Some` here either, so that bailout was the only exit the path had.
///
/// The signal that DOES exist on this path is the writer's own: `FUN_14240fd70`, the SL save-job
/// body, runs for every save the worker performs, bypassed or not. A completion past the value
/// sampled at the fire is positive evidence that a save finished writing, and its result code is
/// the game's own verdict on it. When the observer is not installed there is no such evidence,
/// and the outcome is reported as UNOBSERVED -- which is the truth -- rather than as a failure.
fn save_flow_degraded_commit_wait_tick(ticks: usize, completions_at_fire: u64) {
    let completed = er_save_suppress::save_job_completions() > completions_at_fire;
    if !completed && ticks < SAVE_BYPASS_WATCHDOG_TICKS {
        return;
    }
    // Same interlock as the armed path: never score, and never disarm, while a body is running.
    // Nothing has been consumed at this point, so a deferral simply retries on the next tick.
    if !save_dest_teardown_allowed(completions_at_fire, "degraded commit wait") {
        return;
    }
    let window_was_armed = save_dest_commit_window_armed();
    let verdict = save_dest_verify_and_disarm("degraded commit");
    if window_was_armed && verdict.is_none() {
        return;
    }
    let file_ok = verdict.as_ref().is_none_or(|verdict| verdict.ok);
    let file_state = match verdict.as_ref() {
        Some(verdict) if verdict.ok => "VERIFIED",
        Some(_) => "FAILED",
        None => "not armed",
    };
    let detail = verdict.as_ref().map_or_else(
        || "no commit window was armed, so no file could be checked".to_owned(),
        |verdict| verdict.summary.clone(),
    );
    if completed {
        let result = er_save_suppress::save_job_last_result();
        let status = er_save_suppress::save_job_result_to_status(result);
        let status_ok = status == 0;
        if status_ok && file_ok {
            SAVE_FLOW_DEGRADED_COMPLETE_COUNT.fetch_add(1, Ordering::SeqCst);
            SAVE_FLOW_COMMIT_COMPLETE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-flow: DEGRADED COMMIT COMPLETE after {ticks} commit ticks -- suppression was never armed, so this was an ordinary native save; the SL writer's job body returned with result={result} -> terminal status 0 AND the file VERIFIED on disk: {detail}"
            ));
        } else {
            if status_ok {
                SAVE_FLOW_COMMIT_VERIFY_FAIL_COUNT.fetch_add(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "save-flow: DEGRADED COMMIT FAILED after {ticks} commit ticks -- the SL writer's job body returned with result={result} -> terminal status {status} (0=success), file check {file_state}: {detail}. The user's save did NOT land"
            ));
        }
    } else {
        // No completion inside the watchdog. Say exactly that. A degraded build whose writer
        // observer is absent has no way to see a save finish, and calling that "the save did
        // not happen" would be a claim nothing here can support.
        SAVE_FLOW_DEGRADED_UNOBSERVED_COUNT.fetch_add(1, Ordering::SeqCst);
        SAVE_FLOW_COMMIT_WATCHDOG_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow: DEGRADED COMMIT UNOBSERVED after {ticks} commit ticks -- suppression was never armed and no SL save-job completion arrived (observer installed={}, jobs started/completed since boot={}/{}). Whether the native write happened is UNKNOWN from here; the file check says {file_state}: {detail}",
            er_save_suppress::save_job_observer_installed(),
            er_save_suppress::save_job_starts(),
            er_save_suppress::save_job_completions()
        ));
    }
    save_dest_reset("degraded commit");
    save_flow_enter_stage(
        SAVE_FLOW_STAGE_IDLE,
        if completed && file_ok {
            "degraded commit verified"
        } else {
            "degraded commit unverified"
        },
    );
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
    // OS-PICKER DIM: stand the cover's window up NOW, while nothing is waiting on it. The dialog it
    // covers blocks the menu thread, so the moment it opens is the moment we can no longer afford to
    // be creating a window and a full-screen DIB. Self-gated to sessions that actually run the OS
    // picker (`os_native_save_picker = true`); the in-game browser needs no cover.
    install_picker_dim_overlay();
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

#[cfg(test)]
mod save_flow_deadline_tests {
    use super::*;

    /// Every save-flow bound, referenced by its real constant so a future retune cannot silently
    /// invalidate the proof below. Two entries share `SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS` because
    /// the browser's OPEN and its TEARDOWN are two uses of one budget.
    const SAVE_FLOW_BOUNDS: [usize; 7] = [
        SAVE_FLOW_BOX_BUILD_TIMEOUT_TICKS,
        SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS,
        SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS,
        SAVE_FLOW_FIRE_GATE_TIMEOUT_TICKS,
        SAVE_FLOW_ENQUEUE_GRACE_TICKS,
        SAVE_BYPASS_WATCHDOG_TICKS,
        SAVE_BYPASS_WATCHDOG_TICKS + SAVE_DEST_TEARDOWN_UNPROVEN_EXTRA_TICKS,
    ];

    /// A user may browse for arbitrarily long, so the freeze cannot be "extend the timeouts". Run
    /// the frozen read past the LONGEST bound in the flow (~75 s of game-task frames at 60 Hz) and
    /// assert it never reaches any of them; then one unfrozen call must advance by exactly 1, so the
    /// freeze is a suspension and not a break.
    #[test]
    fn a_frozen_counter_crosses_no_save_flow_bound() {
        let counter = AtomicUsize::new(0);
        let iterations = SAVE_BYPASS_WATCHDOG_TICKS + SAVE_DEST_TEARDOWN_UNPROVEN_EXTRA_TICKS + 1;
        for frame in 0..iterations {
            let ticks = save_flow_next_stage_ticks(true, &counter);
            assert_eq!(ticks, 0, "the frozen read accrued at frame {frame}");
            for bound in SAVE_FLOW_BOUNDS {
                assert!(
                    ticks < bound,
                    "frame {frame} reached the {bound}-tick bound while a dialog was open"
                );
            }
        }
        assert_eq!(
            save_flow_next_stage_ticks(false, &counter),
            1,
            "the counter must resume from where it was frozen, advancing by exactly one"
        );
    }

    /// FREEZING IS NOT ENOUGH. Stage 3's abandon branch never consulted `ticks`, so with the dialog
    /// term missing it would end the flow one frame after the dialog opened no matter how frozen
    /// the counter was. Both halves are asserted here: with the term, every tick count waits; with
    /// it removed, the same state is abandoned.
    #[test]
    fn an_open_os_dialog_is_never_read_as_an_abandoned_browser() {
        for ticks in [
            1,
            SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS - 1,
            SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS,
            SAVE_FLOW_FIRE_GATE_TIMEOUT_TICKS,
            SAVE_BYPASS_WATCHDOG_TICKS,
            100_000,
        ] {
            assert_eq!(
                dest_browse_verdict(false, false, false, true, false, false, ticks),
                DestBrowseAction::WaitForUser,
                "an open OS dialog must have no deadline at tick {ticks}"
            );
            // The ordering window: the menu-pump arm sets the dialog latch BEFORE clearing the
            // pending-open latch, so a tick landing between the two stores sees both set.
            assert_eq!(
                dest_browse_verdict(false, false, false, true, false, true, ticks),
                DestBrowseAction::WaitForUser,
                "the both-latches-set window must not time out at tick {ticks}"
            );
        }
        assert_eq!(
            dest_browse_verdict(false, false, false, false, false, false, 1),
            DestBrowseAction::Abandoned,
            "without the dialog term the very same state aborts on the next frame -- this is the \
             bug the tick freeze alone does not fix"
        );
    }

    /// Game-task ticks that accrue between one destination dialog closing and the menu pump opening
    /// the next. Measured from the loop in bd `er-effects-rs-rsxi`: CLOSED -> OPENED was 55-85 ms,
    /// and `SAVE_FLOW_STAGE_TICKS` is frozen for the dialog's own lifetime, so only that gap counts.
    const REOPEN_GAP_TICKS: usize = 3;

    /// THE REOPEN LOOP, REPRODUCED (bd `er-effects-rs-rsxi`, measured 2026-07-30 on
    /// `surface=save-as`: OPENED -> `result=cancelled` -> OPENED again 57 ms later, over and over,
    /// each cancel logging "nothing staged" while the next pump re-asked).
    ///
    /// Nothing in the verdict function was wrong -- the defect sat one actor EARLIER, in who owns
    /// `SAVE_DEST_OPEN_PICKER_PENDING`. So the model here is the TWO actors, not one predicate: the
    /// menu pump opens a dialog whenever the request is armed, and the save-flow tick then judges
    /// the latches. Counting the dialogs a cancelling user is shown is the whole bug report as a
    /// number.
    ///
    /// The old behaviour is not literally infinite -- `OpenTimeout` fires once the stage has accrued
    /// its budget -- but ~60 dialogs is indistinguishable from a trap, and it is a FLOOR, because
    /// every Box3 round-trip re-enters stage 3 and resets the budget to zero.
    #[test]
    fn a_cancelling_user_is_shown_one_destination_dialog_not_a_reopen_loop() {
        /// One run of the pump+tick pair against a user who cancels every dialog. Returns how many
        /// dialogs were opened before the flow reached a terminal verdict.
        fn dialogs_shown(discharge_on_dismissal: bool) -> usize {
            let mut armed = true;
            let mut shown = 0usize;
            let mut ticks = 0usize;
            loop {
                if armed {
                    // MENU PUMP: the request is armed, so comdlg32 opens. The user cancels it, and
                    // the tick counter is frozen for the dialog's whole lifetime.
                    shown += 1;
                    assert!(shown < 1000, "neither behaviour may run away in this model");
                    if discharge_on_dismissal && PickerOpenOutcome::Dismissed.request_discharged() {
                        armed = false;
                    }
                }
                ticks += REOPEN_GAP_TICKS;
                // SAVE-FLOW TICK: no dialog is up by now, nothing is committed or confirmed.
                match dest_browse_verdict(false, false, false, false, false, armed, ticks) {
                    DestBrowseAction::WaitForUser => continue,
                    DestBrowseAction::Abandoned | DestBrowseAction::OpenTimeout => return shown,
                    other => panic!("a cancelling user cannot reach {other:?}"),
                }
            }
        }
        assert_eq!(
            dialogs_shown(true),
            1,
            "one Cancel must produce exactly one dialog and then end the flow"
        );
        let trapped = dialogs_shown(false);
        assert!(
            trapped > 50,
            "the shipped behaviour re-asked the cancelled request every pump; this model got out \
             after only {trapped} dialogs, so it no longer reproduces the reported trap"
        );
    }

    /// A confirm the OS arm staged must reach Box3 even though nothing is live by then: its dialog
    /// closed before the target was known, so a verdict that consulted liveness first would call it
    /// abandoned and silently drop the save.
    #[test]
    fn an_owed_overwrite_confirm_outranks_the_abandoned_verdict() {
        assert_eq!(
            dest_browse_verdict(false, false, false, false, true, false, 1),
            DestBrowseAction::EnterBox3
        );
        assert_eq!(
            dest_browse_verdict(false, false, false, false, true, false, 100_000),
            DestBrowseAction::EnterBox3,
            "an owed confirm has no deadline either"
        );
        assert_eq!(
            dest_browse_verdict(true, false, false, false, true, false, 1),
            DestBrowseAction::CloseAndCommit,
            "a destination already committed still outranks an owed confirm"
        );
    }

    /// The in-game verdicts, unchanged. With the OS terms both false this reproduces the shipping
    /// branch order exactly, so the extraction is provably behaviour-preserving for the default.
    #[test]
    fn the_in_game_verdicts_are_what_they_were_before_the_os_terms_existed() {
        let over = SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS;
        let under = over - 1;
        for (label, args, expected) in [
            (
                "commit staged, browser gone",
                (true, false, false, false, false, false, 1),
                DestBrowseAction::CloseAndCommit,
            ),
            (
                "commit staged, browser still up, under budget",
                (true, true, false, false, false, false, under),
                DestBrowseAction::WaitForUser,
            ),
            (
                "commit staged, browser will not tear down",
                (true, true, false, false, false, false, over),
                DestBrowseAction::TeardownTimeout,
            ),
            (
                "browser live, user choosing",
                (false, true, true, false, false, false, 100_000),
                DestBrowseAction::WaitForUser,
            ),
            (
                "open staged, not up yet, under budget",
                (false, false, false, false, false, true, under),
                DestBrowseAction::WaitForUser,
            ),
            (
                "open staged, never appeared",
                (false, false, false, false, false, true, over),
                DestBrowseAction::OpenTimeout,
            ),
            (
                "nothing live, nothing pending",
                (false, false, false, false, false, false, 1),
                DestBrowseAction::Abandoned,
            ),
        ] {
            let (commit, window, dest_mode, os_open, confirm, open_pending, ticks) = args;
            assert_eq!(
                dest_browse_verdict(
                    commit,
                    window,
                    dest_mode,
                    os_open,
                    confirm,
                    open_pending,
                    ticks
                ),
                expected,
                "in-game verdict changed for: {label}"
            );
        }
    }
}
