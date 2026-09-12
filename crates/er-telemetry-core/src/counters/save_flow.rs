//! The System->Quit "Save Game" flow: its stage machine, confirm boxes and destination
//! browser.
//!
//! The flow closes the dialog before it fires the native save request, so the stages
//! below are the only record of where a run got to: the confirm boxes, the browse that
//! picks where the copy lands, the fire gate that refuses a latched
//! `SaveRequest_Profile`, and the commit watchdog that tells a completed write apart from
//! one nothing observed.
//!
//! Split out of `counters.rs` as a pure code move: nothing is renamed and no initial value
//! changes. Every name here is re-exported from `er_telemetry_core::counters` with a glob, so
//! each consumer still spells it `er_telemetry_core::counters::<name>`.

use std::sync::atomic::{AtomicUsize, Ordering};

// ---- save-flow (System->Quit "Save Game" close-then-fire commit; save-game-flow WP1) ----
/// Save-flow stage machine value, exported as `oracle_save_flow_stage`. Stage map:
/// 0 idle, 1 BOX1_WAIT (WP2), 2 BOX2_WAIT (WP2), 3 DEST_BROWSE (WP3), 4 BOX3_WAIT (WP3),
/// 5 CLOSING_ABORT (WP2), 6 CLOSING_COMMIT, 7 FIRE_GATE_WAIT, 8 COMMIT_WAIT.
pub static SAVE_FLOW_STAGE: AtomicUsize = AtomicUsize::new(0);
/// Game-task ticks spent in the current save-flow stage (reset on every transition; drives
/// the stage-7 fire-gate timeout and the stage-8 commit watchdog).
pub static SAVE_FLOW_STAGE_TICKS: AtomicUsize = AtomicUsize::new(0);
/// The System/Quit tab PropertyEditDialog captured at the Save Game row press (diagnostic
/// correlation pointer; WP2/WP3 reuse it as the confirm-box submit context).
pub static SAVE_FLOW_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Times the stage-7 fire gate found the CSMenuMan[+0x80] +0x290/+0x298 failure latch set.
/// Latched means SaveRequest_Profile's gate fails permanently for the session, so the flow
/// aborts instead of firing (exported as `oracle_save_flow_gate_latch_blocked`).
pub static SAVE_FLOW_GATE_LATCH_BLOCKED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Completed Save Game commits: the bypassed save reported terminal status 0 (success) and the
/// file it was supposed to produce verified on disk. The file check is part of the condition on
/// purpose -- the SL status is the game's opinion of its own job and says nothing about bytes.
pub static SAVE_FLOW_COMMIT_COMPLETE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Save Game row presses. One bypass arm and one commit are expected per press, so this is what
/// tells a double-arm bug from a user who simply pressed the row twice.
pub static SAVE_FLOW_ROW_PRESS_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Commits where the game reported terminal status 0 but the on-disk check FAILED: the save was
/// announced as successful and no usable file exists. Non-zero is a hard product failure.
pub static SAVE_FLOW_COMMIT_VERIFY_FAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::bypass_allowed_total()` sampled at the instant the forced save request was
/// fired. Stage 8 compares against it to tell "the save enqueue ARRIVED and is being written"
/// (total advanced -> a real write is in flight, protect it) from "the enqueue never arrived"
/// (total unchanged -> nothing is in flight and the flow is already dead). Without that
/// distinction stage 8 held the Save Game row hostage for the full watchdog even when the fire
/// had silently failed.
pub static SAVE_FLOW_BYPASS_ALLOWED_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// Save flows whose forced request never produced a save enqueue: the fire reached the native
/// request flags but no SL save arrived at the suppressor inside the grace window. The user's
/// save did not happen -- a hard failure oracle, distinct from a user-declined abort.
pub static SAVE_FLOW_ENQUEUE_MISSING_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Commits that ended on the stage-8 watchdog instead of on an observed outcome. The
/// watchdog is a BACKSTOP: it expires a stranded token and frees the UI, but it never learns
/// what happened, so every one of these is a degraded commit even when the file turns out to
/// be fine. Non-zero means the write-completion signal did not reach the flow and the reason
/// has to be found -- silence here used to be indistinguishable from success.
pub static SAVE_FLOW_COMMIT_WATCHDOG_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::save_job_starts()` sampled at the fire. Stage 8 compares against it to
/// timestamp the tick the SL worker actually began writing.
pub static SAVE_FLOW_SAVE_JOB_STARTS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// Commit tick on which the SL worker was first seen to have started writing (0 = not yet /
/// never). Read beside the tick count in the completion line: the difference between them is
/// how long the native write itself took, and the rest is how long the flow took to notice.
pub static SAVE_FLOW_COMMIT_JOB_START_TICK: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::dispatch_calls()` sampled at the fire. A stage-8 failure compares
/// against it to say whether the native save dispatcher ran at all after the request flags
/// were set -- the difference between "nothing consumed the request" and "the dispatcher
/// consumed it and refused", which the enqueue-side counters alone cannot distinguish.
pub static SAVE_FLOW_DISPATCH_CALLS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::dispatch_declines()` sampled at the fire.
pub static SAVE_FLOW_DISPATCH_DECLINES_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::serialize_failures()` sampled at the fire. A post-fire increase means
/// the character serializer `FUN_14067dc00` is what refused, which is upstream of both the
/// submit builder and the suppressor.
pub static SAVE_FLOW_SERIALIZE_FAILURES_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::serialize_calls()` sampled at the fire. This is the allocation oracle:
/// both character lanes allocate their MainHeap buffers (`0x280000`, plus `0x60000` on the
/// combined lane) and null-check them before calling `FUN_14067dc00`, so a post-fire
/// increase proves the allocations succeeded and the lane got as far as the serializer. No
/// increase, with declines climbing, means the lane bailed earlier -- an allocation
/// returned null, or one of the pre-allocation gates (`CanShowSaveMenu()`, `saveState != 0`,
/// slot index >= 10) turned it away.
pub static SAVE_FLOW_SERIALIZE_CALLS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `er_save_suppress::submits_swallowed()` sampled at the fire. A post-fire increase with no
/// bypass allow means a submit was built and this DLL swallowed it by mistake -- the one
/// failure mode where the fault is ours rather than the game's.
pub static SAVE_FLOW_SUBMITS_SWALLOWED_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
/// `GameMan+0xb72` / `+0xb73` sampled immediately before the forced request pair is fired.
///
/// This is what makes a retraction scoped rather than a broad clear. A flag that was
/// already set before our fire belongs to the game and is left alone; only a flag that went
/// 0 -> 1 across our own call is ours to take back. Stored as the raw byte, or
/// [`SAVE_FLOW_FLAG_UNREAD`] when GameMan was not readable at the fire -- which disqualifies
/// the retraction for that flag, because "we could not see it" is not "it was clear".
pub static SAVE_FLOW_B72_BEFORE_FIRE: AtomicUsize = AtomicUsize::new(SAVE_FLOW_FLAG_UNREAD);
/// `GameMan+0xb73` sampled immediately before the forced request pair. See
/// [`SAVE_FLOW_B72_BEFORE_FIRE`].
pub static SAVE_FLOW_B73_BEFORE_FIRE: AtomicUsize = AtomicUsize::new(SAVE_FLOW_FLAG_UNREAD);
/// Sentinel for a request flag that could not be read at the fire.
pub const SAVE_FLOW_FLAG_UNREAD: usize = usize::MAX;
/// Save-request flags this DLL retracted after its own fire provably went nowhere.
///
/// Each retraction ends a per-frame spin: a refused save lane touches nothing, so the
/// dispatcher re-enters it every frame and re-serializes the whole character (0x280000
/// bytes) forever. Measured at ~33 serializations/second on a stuck run. Non-zero here is
/// the flow cleaning up after itself, not an error.
pub static SAVE_FLOW_REQUEST_RETRACTIONS: AtomicUsize = AtomicUsize::new(0);
/// Retractions the flow declined to perform because it could not prove the latched flags
/// were its own (or because a return-title sequence was in flight and needs the request to
/// survive). Non-zero means a spin was left running on purpose; read it beside
/// `oracle_save_dispatch_declines` to see the cost.
pub static SAVE_FLOW_RETRACT_DECLINED: AtomicUsize = AtomicUsize::new(0);

// ---- save-flow confirm box (save-game-flow WP2, reduced to one box 2026-07-31) ----
/// Number of confirm boxes the save flow can build. It is ONE: "Overwrite this file?", asked
/// only when the chosen destination already exists. The two up-front confirms this flow used to
/// open ("Are you sure you want to save?" and "Overwrite your loaded save?") were removed -- they
/// asked the user to predict a destination before seeing the list, which is the mistake the
/// reviewer reported. Indexes the per-box counters below; box ids are 1-based so 0 stays the
/// "no box" sentinel.
pub const SAVE_FLOW_BOX_COUNT: usize = 1;
/// Box id (1..=SAVE_FLOW_BOX_COUNT) the next `CS::MessageBoxDialog` build belongs to, set
/// immediately before the confirm-box MenuJob is submitted and cleared by the builder hook
/// that captures the dialog. Non-zero makes the builder hook forward the build and capture
/// it into `SAVE_FLOW_BOX_DIALOG` instead of applying the product msgbox suppression.
pub static SAVE_FLOW_BOX_EXPECTED: AtomicUsize = AtomicUsize::new(0);
/// The captured confirm-box `CS::MessageBoxDialog` (0 = none live). Deliberately a dedicated
/// slot: `MSGBOX_LAST_DIALOG` / `CONNECTION_ERROR_DIALOG` feed the startup auto-accept, which
/// must never touch a user-facing save confirm.
pub static SAVE_FLOW_BOX_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Menu-pump pending: build+submit this confirm box id from `system_quit_menu_window_run_post`
/// (the proven menu-job submit context). 0 = nothing pending.
pub static SAVE_FLOW_SUBMIT_BOX_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Confirm boxes whose dialog was captured, per box id - 1.
pub static SAVE_FLOW_BOX_OPEN_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Affirmative decisions per box id - 1.
pub static SAVE_FLOW_BOX_YES_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Negative/cancel decisions per box id - 1.
pub static SAVE_FLOW_BOX_NO_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Save flows that ended back in the world with nothing written (user said No/cancel, or a
/// recipe failure aborted the chain).
pub static SAVE_FLOW_ABORT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// UNDECIDABLE confirm boxes, per box id - 1: the dialog stopped being a MessageBoxDialog
/// (freed/reused) or reported that it had emitted a result we could not map to a button. These
/// are failures, deliberately kept out of the No counters: a box we could not read is not the
/// user pressing No, and conflating the two makes an agent-invented answer indistinguishable
/// from a real one. An undecidable box always ends the flow without writing.
pub static SAVE_FLOW_BOX_UNDECIDABLE_COUNTS: [AtomicUsize; SAVE_FLOW_BOX_COUNT] =
    [const { AtomicUsize::new(0) }; SAVE_FLOW_BOX_COUNT];
/// Times a captured confirm-box dialog failed its structural identity check (its vtable no
/// longer carries `CS::MessageBoxDialog::Update` in slot 2) -- the object was freed or reused
/// while we were polling it. Subset of `SAVE_FLOW_BOX_UNDECIDABLE_COUNTS`.
pub static SAVE_FLOW_BOX_IDENTITY_LOST_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `CS::MenuJob::EmitResult` observations attributed to the live confirm box: the emitted
/// `MenuJobResult` state (2 = Success/Yes, 3 = Failed/No or cancel) plus the dialog it came
/// from, latched by the emit hook for the save-flow poll. `..._DIALOG` 0 = nothing captured.
pub static SAVE_FLOW_BOX_EMIT_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_FLOW_BOX_EMIT_STATE: AtomicUsize = AtomicUsize::new(0);
/// Emitted-result observations for the live confirm box (diagnostic count).
pub static SAVE_FLOW_BOX_EMIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The confirm box's `MenuJobResult` state as built, sampled the moment the builder hook
/// captures the dialog (stored as a `u32` bit pattern). The poll only believes that field
/// once it has changed away from this baseline, and refuses to use it at all when the
/// baseline is already terminal -- the discipline that the 2026-07-28 defect was missing,
/// where a value present at construction was mistaken for the user's answer.
pub static SAVE_FLOW_BOX_RESULT_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Install flag for the `CS::MenuJob::EmitResult` observer hook (0 = not installed).
pub static MENU_JOB_EMIT_RESULT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Submitted confirm boxes whose `CS::MessageBoxDialog` build was never captured within
/// `SAVE_FLOW_BOX_BUILD_TIMEOUT_TICKS` -- the recipe produced no visible box (failure path).
pub static SAVE_FLOW_BOX_BUILD_TIMEOUT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once a MessageBoxBuilder recipe RVA failed its prologue byte check: the overwrite confirm
/// cannot be built on this build. Save Game still opens the destination list (that needs no
/// message box), and a destination that would OVERWRITE an existing file is refused rather than
/// written unconfirmed -- a free name still commits.
pub static SAVE_FLOW_RECIPE_UNAVAILABLE: AtomicUsize = AtomicUsize::new(0);
/// Destination picks refused because the overwrite confirm could not be built on this build.
/// Non-zero means a user chose an existing file and nothing was written; the free-name path is
/// unaffected.
pub static SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Dialog the confirm box is built against and submitted to (its MenuJob queue at +0x10 and
/// MenuWindow list at +0x50). 0 = the System/Quit dialog captured at the row press. The in-game
/// destination browser sets the live `05_010` ProfileLoadDialog instead, exactly like the game's
/// own load-confirm (`FUN_1409a4670` submits its confirm to `profile_load_dialog+0x10`), so the
/// confirm never contends with the System dialog queue that still owns the open picker window job.
pub static SAVE_FLOW_BOX_HOST_DIALOG: AtomicUsize = AtomicUsize::new(0);

// ---- save destination browser (save-game-flow WP3) ----
/// 1 while the live `05_010` picker is the save-destination chooser (the Save Game row opens it
/// directly) rather than the load-source browser. Cleared by `save_picker_reset` like the rest of the picker latches.
pub static SAVE_PICKER_DEST_MODE: AtomicUsize = AtomicUsize::new(0);
/// System/Quit PropertyEditDialog the live picker window was submitted from. The load-source
/// picker resolves it from its row action object; the destination picker is opened by the save
/// flow, which has the dialog but no action object. The menu-pump resubmit reopens through it.
pub static SAVE_PICKER_SYSTEM_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Menu-pump pending: open the destination browser from `system_quit_menu_window_run_post` (the
/// proven menu-job submit context). Set by the Save Game row press, and again whenever the OS
/// surface has to re-show its Save-As after a declined overwrite.
pub static SAVE_DEST_OPEN_PICKER_PENDING: AtomicUsize = AtomicUsize::new(0);
/// Times the menu pump tried to open the destination browser and left the request armed because no
/// picker ran (a MenuJob the dialog's queue deferred). The direct oracle for the reopen loop of bd
/// `er-effects-rs-rsxi`: a picker that ran -- including one the user cancelled -- discharges the
/// request, so with the OS surface active this must read 0. Any positive value there means a
/// terminal outcome was retried, which is the loop that trapped the user.
pub static SAVE_DEST_PICKER_OPEN_RETRY_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once a destination has been chosen and confirmed: the save-flow tick closes the menus with
/// the commit staged as soon as the picker window has finished tearing down.
pub static SAVE_DEST_COMMIT_PENDING: AtomicUsize = AtomicUsize::new(0);
/// 1 while the scoped write-open redirect window is armed (`oracle_save_dest_redirect_armed`):
/// a `CreateFileW` write-open of the live save leaf is diverted to the chosen destination.
pub static SAVE_DEST_REDIRECT_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Write-opens diverted to the destination during the armed window. One per dirty block, not one
/// per commit: the native save takes the in-place path (`FUN_1424142e0`) whenever every supplied
/// block still fits its existing entry, and that opens the container once per block. Only the full
/// rebuild (`FUN_142413860`) is a single whole-buffer write. Measured 2026-07-28: 2 hits for one
/// Save Game commit. Zero hits is the failure -- any positive count is normal.
pub static SAVE_DEST_REDIRECT_HITS: AtomicUsize = AtomicUsize::new(0);
/// Destinations pre-seeded with a byte copy of the live container before the fire. The in-place
/// block writer seeks to offsets read from the live index, so an unseeded destination receives a
/// sparse fragment instead of a save.
pub static SAVE_DEST_SEEDED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Commits aborted because the destination seed could not be written. The request is not fired.
pub static SAVE_DEST_SEED_FAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once this commit's verified destination parsed as a structurally complete BND4 container: its
/// own entry index accounts for every byte up to EOF. This is the check that separates a loadable
/// container from a file that merely has the right length. Per-commit: cleared by
/// [`save_dest_reset_commit_verdicts`] at every arm.
pub static SAVE_DEST_TARGET_STRUCTURE_OK: AtomicUsize = AtomicUsize::new(0);
/// Commits whose destination is the loaded save -- a browsed pick (or `[ new ]` in the loaded
/// save's own folder) that resolves back to it. Non-zero means this flow deliberately rewrote the user's live save file -- the only
/// sanctioned way that happens, and the counter that keeps such a rewrite from reading as an
/// anonymous mutation or a suppression leak.
pub static SAVE_DEST_LIVE_OVERWRITE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 if the loaded save's `.bak` twin moved during this destination commit. Not a failure: the
/// native backup step (`FUN_142410830`) is not redirected and can only copy the untouched live
/// container over its own backup. Named so the movement is never unattributed. Per-commit: cleared
/// by [`save_dest_reset_commit_verdicts`] at every arm.
pub static SAVE_DEST_LIVE_BAK_MUTATED: AtomicUsize = AtomicUsize::new(0);
/// Destination browsers opened (one per Save Game row press, plus a re-open after a declined
/// overwrite on the OS surface).
pub static SAVE_DEST_PICKER_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination picks that landed on an existing file (a pre-existing row, or `[ new ]` whose
/// filename already exists in the browsed folder) -- these go through the Box3 overwrite confirm.
pub static SAVE_DEST_TARGET_EXISTING_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination picks that created a new file via `[ new ]` (no Box3; nothing is overwritten).
pub static SAVE_DEST_TARGET_NEW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination commits staged (a target was chosen and confirmed; the menus are closing).
pub static SAVE_DEST_COMMIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Destination browsers abandoned (backed out / closed without choosing) -- nothing is written.
pub static SAVE_DEST_CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once this destination commit verified: the target exists, starts with `BND4`, matches the live
/// container size, and changed on disk during the armed window. Per-commit: cleared by
/// [`save_dest_reset_commit_verdicts`] at every arm. Cumulative failure history lives in
/// [`SAVE_DEST_COMMIT_FAIL`].
pub static SAVE_DEST_TARGET_WRITTEN_OK: AtomicUsize = AtomicUsize::new(0);
/// Destination commits whose target verification failed (missing/short/unchanged target, or zero
/// redirect hits): the user's save did not land where they asked. Cumulative for the process.
pub static SAVE_DEST_COMMIT_FAIL: AtomicUsize = AtomicUsize::new(0);
/// 1 if the live save file changed during this destination commit -- the redirect leaked and the
/// loaded save was overwritten anyway. Hard failure: the pre-fire snapshot is restored over it.
/// Per-commit: cleared by [`save_dest_reset_commit_verdicts`] at every arm, with the process-wide
/// count kept in [`SAVE_DEST_LIVE_FILE_MUTATED_TOTAL`].
pub static SAVE_DEST_LIVE_FILE_MUTATED: AtomicUsize = AtomicUsize::new(0);
/// Every leak this process ever saw: incremented, never cleared, wherever
/// [`SAVE_DEST_LIVE_FILE_MUTATED`] is raised. The per-commit flag has to be cleared at arm time or
/// one leak condemns every later commit, and a leak that the snapshot restore then repaired
/// increments no other cumulative counter -- so without this the worst event the flow can produce
/// would be erasable by the next arm.
pub static SAVE_DEST_LIVE_FILE_MUTATED_TOTAL: AtomicUsize = AtomicUsize::new(0);
// ---- Destination-commit SAFETY ORACLES (2026-07-29) ----
// Every counter below names a refusal, a deferral, or a fact the commit could not establish.
// They exist because the previous shape of this flow could destroy the loaded save while its
// log read "restored pre-fire snapshot ok=true": a decision it got wrong had no name, so no
// run could report it. Each of these is that missing name.
/// Commits refused because the destination could not be proven either identical to, or distinct
/// from, the loaded save (a handle-identity probe that neither succeeded nor said "absent").
/// Firing on an unproven answer is what turns a save into a self-redirect that restores the
/// pre-save snapshot over the save that just succeeded, so the commit refuses instead.
pub static SAVE_DEST_IDENTITY_UNKNOWN_ABORT: AtomicUsize = AtomicUsize::new(0);
/// Browsed destinations proven to be the loaded save by handle identity while their path strings
/// differed (the Wine `C:\users\steamuser\...` vs `Z:\...\pfx\drive_c\users\steamuser\...`
/// spelling of one file). Non-zero means a self-redirect was blocked and the commit took the
/// sanctioned overwrite-the-loaded-save path instead.
pub static SAVE_DEST_SELF_REDIRECT_BLOCKED: AtomicUsize = AtomicUsize::new(0);
/// Destination commits refused because the SL save-job body observer is not installed. Without
/// it the writer's completion is unobservable, and the redirect window would have to be torn
/// down on a tick count -- which can close it between two of the in-place writer's per-block
/// opens and patch the rest into the loaded save.
pub static SAVE_DEST_NO_WRITER_OBSERVER_ABORT: AtomicUsize = AtomicUsize::new(0);
/// Write-opens of a save-container leaf that were not diverted because their directory is not
/// the loaded save's. Every one of these would previously have been rewritten into the user's
/// chosen destination purely because its file name matched.
pub static SAVE_DEST_FOREIGN_OPEN_PASSED: AtomicUsize = AtomicUsize::new(0);
/// Teardown attempts deferred because the native writer was still inside a save-job body. The
/// redirect window must span every one of the in-place writer's per-block opens.
pub static SAVE_DEST_DISARM_DEFERRED: AtomicUsize = AtomicUsize::new(0);
/// Redirect windows torn down without positive evidence that the writer ever ran (the enqueue
/// was forwarded, no job body ever started, and the extended teardown bound elapsed). A failure
/// oracle: the commit is over and nothing can say whether the writer will still appear.
pub static SAVE_DEST_DISARM_UNPROVEN: AtomicUsize = AtomicUsize::new(0);
/// 1 if the loaded save's stat could not be read at this commit's verification. Distinct from
/// [`SAVE_DEST_LIVE_FILE_MUTATED`]: unreadable is not changed, and treating it as changed is
/// what triggered a blind whole-container overwrite of the live save on a transient stat error.
/// Per-commit: cleared by [`save_dest_reset_commit_verdicts`] at every arm; every occurrence also
/// increments the cumulative [`SAVE_DEST_RESTORE_SUPPRESSED`].
pub static SAVE_DEST_LIVE_STAT_UNREADABLE: AtomicUsize = AtomicUsize::new(0);
/// Restores of the pre-fire snapshot that were DECLINED: the loaded save's stamp moved but its
/// bytes are unchanged, its stat is unreadable, or the destination turned out to be the same
/// file. Writing the snapshot in any of those cases destroys rather than protects.
pub static SAVE_DEST_RESTORE_SUPPRESSED: AtomicUsize = AtomicUsize::new(0);
/// Restores that were attempted and failed. The restore is temp-file + rename, so a failure
/// leaves the loaded save byte-for-byte as the writer left it rather than truncated.
pub static SAVE_DEST_RESTORE_FAILED: AtomicUsize = AtomicUsize::new(0);
/// Every 0/1 oracle that describes one destination commit, in a single list so the arm-time reset
/// and the per-commit export can never disagree about which oracles those are.
///
/// Each is stored only on the branch that observes it and left untouched otherwise, so nothing
/// clears them by itself: after one verified commit `oracle_save_dest_target_written_ok` and
/// `..._structure_ok` stayed 1 for the life of the process, and every later failed commit
/// published a save that did not land as a save that did -- the one direction a proof oracle must
/// never fail in. The mutation/unreadable oracles latch the same way in reverse and condemn a good
/// commit for an earlier one's leak.
pub fn save_dest_commit_verdict_oracles() -> [&'static AtomicUsize; 6] {
    [
        &SAVE_DEST_REDIRECT_HITS,
        &SAVE_DEST_TARGET_WRITTEN_OK,
        &SAVE_DEST_TARGET_STRUCTURE_OK,
        &SAVE_DEST_LIVE_FILE_MUTATED,
        &SAVE_DEST_LIVE_BAK_MUTATED,
        &SAVE_DEST_LIVE_STAT_UNREADABLE,
    ]
}

/// Clear the previous commit's verdict, so what is exported is always this commit's result.
///
/// Called from every arm site (`save_dest_arm_redirect`, `save_dest_arm_live_overwrite`) -- the
/// point at which a commit becomes the one being scored. Cumulative history is deliberately not
/// reset with it: [`SAVE_DEST_COMMIT_FAIL`], [`SAVE_DEST_RESTORE_SUPPRESSED`],
/// [`SAVE_DEST_RESTORE_FAILED`] and [`SAVE_DEST_LIVE_FILE_MUTATED_TOTAL`] span the whole process,
/// so a run still reports every failure it ever had alongside the current verdict.
pub fn save_dest_reset_commit_verdicts() {
    for oracle in save_dest_commit_verdict_oracles() {
        oracle.store(0, Ordering::SeqCst);
    }
}
/// 1 when the current commit was fired on the degraded fail-open path (suppression never armed,
/// so no bypass token exists). These are real native saves; they are completed on the writer's
/// own job-completion signal, never on the token-consumption test, which can never move here.
/// Rewritten at every fire, so it always describes the commit stage 8 is waiting on.
pub static SAVE_FLOW_DEGRADED_FIRE: AtomicUsize = AtomicUsize::new(0);
/// Degraded-path commits that reached a verdict on the writer's job-completion signal.
pub static SAVE_FLOW_DEGRADED_COMPLETE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Degraded-path commits that ended with the write UNOBSERVED (no job-completion observer, or
/// none arrived inside the watchdog). Reported as degraded, never as "the save did not happen".
pub static SAVE_FLOW_DEGRADED_UNOBSERVED_COUNT: AtomicUsize = AtomicUsize::new(0);
/// [`er_save_suppress::save_job_completions`] sampled immediately before the forced request was
/// fired. The teardown gate and the degraded completion test are both relative to this.
pub static SAVE_FLOW_SAVE_JOB_COMPLETIONS_AT_FIRE: AtomicUsize = AtomicUsize::new(0);
