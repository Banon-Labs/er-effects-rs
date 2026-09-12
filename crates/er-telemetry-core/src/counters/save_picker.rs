//! Every surface that picks a save, and the dim cover that owns the file dialog's
//! z-order.
//!
//! Four surfaces answer one question. The in-game rows (`SAVE_PICKER_*`) stage a
//! container into the native list; the OS file dialog (`SAVE_PICKER_OS_*`) counts dialogs
//! and is shared by all three intents; the missing-save boot pick
//! (`SAVE_PICKER_OS_BOOT_*`) counts the boot intent's own outcomes, where a cancel quits
//! the game rather than backing out of a menu; and the dim overlay (`SAVE_PICKER_DIM_*`)
//! is the borderless window the dialog is owned to, so its stacking over the game is a
//! measurement rather than a race.
//!
//! Split out of `counters.rs` as a pure code move: nothing is renamed and no initial value
//! changes. Every name here is re-exported from `er_telemetry_core::counters` with a glob, so
//! each consumer still spells it `er_telemetry_core::counters::<name>`.

use std::sync::atomic::AtomicUsize;

pub static SAVE_PICKER_MODE_ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_REOPEN_PENDING: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OPEN_SLOTS_PENDING: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_ACTION_OBJ: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_REPOPULATE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_PICK_REJECT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_RESUBMIT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_STAGED_ROW_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Times the game's own `CS::ProfileSummary` records were restored over the picker's staged
/// browse-row labels (telemetry oracle `oracle_save_picker_row_records_restored`; cumulative).
///
/// Pair it with `oracle_save_picker_open_count`. The defect this counter exists to make visible was
/// diagnosed by absence -- a 600 MB debug log with pickers opening and not one restore line -- and
/// absence is exactly what a counter turns into a number. Fewer restores than picker opens means
/// staged labels were left in a game-owned structure, which is what put `[..] EldenRing` and
/// `[ new ]` on the user's loading screens as character names.
pub static SAVE_PICKER_ROW_RECORDS_RESTORED: AtomicUsize = AtomicUsize::new(0);
/// Of those restores, the ones that could not write the snapshot back because the live
/// `CS::ProfileSummary` allocation is no longer the one it was taken from
/// (`oracle_save_picker_row_records_restore_unwritable`). The latch is still cleared -- writing a
/// dead allocation's image into whatever now occupies the address would be worse than losing it --
/// so a non-zero here means some records stayed stale and the restore knew it.
pub static SAVE_PICKER_ROW_RECORDS_RESTORE_UNWRITABLE: AtomicUsize = AtomicUsize::new(0);
/// Staged-row restores that were postponed because `GameDataMan+0x78` read as 0 -- the live
/// summary is unreadable this frame, which is normal through the clean-title window
/// (`oracle_save_picker_row_records_restore_deferred`). The snapshot stays armed and the per-frame
/// sweep retries, so a non-zero here is expected and only interesting beside a non-zero
/// `_restore_unwritable`.
pub static SAVE_PICKER_ROW_RECORDS_RESTORE_DEFERRED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_REBUILD_PENDING_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_LIST_BUILDER_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_LIST_BUILDER_RESTAGE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Which file-picker surface this session runs: 0 = the in-game `05_010` browser (default),
/// 1 = the OS common file dialog (`er-quickload.toml os_native_save_picker = true`).
///
/// A latch set once from `init_runtime_config`, not a lazy read, so it is exported even in a
/// session where no picker ever opens. Every other `SAVE_PICKER_OS_*` counter is only meaningful
/// once this reads 1, and a report can state the mode without the reporter knowing the config.
pub static SAVE_PICKER_SURFACE: AtomicUsize = AtomicUsize::new(0);
/// 1 while an OS common file dialog is up and blocking the thread that owns the menu pump.
///
/// One word of state doing triple duty: the re-entrancy claim (taken by compare-exchange, so only
/// the first caller proceeds and a message comdlg32 dispatches back into our own row-action detour
/// cannot open a second dialog), the freeze predicate for `SAVE_FLOW_STAGE_TICKS`, and the stage-3
/// "a browser is live" term. Released by a guard whose `Drop` clears it, so an unwind cannot leave
/// it stuck.
pub static SAVE_PICKER_OS_DIALOG_OPEN: AtomicUsize = AtomicUsize::new(0);
/// Game-task ticks whose `SAVE_FLOW_STAGE_TICKS` accrual was suppressed because a dialog was open.
///
/// Load-bearing, and the only thing that answers a question nothing static can: `> 0` proves the
/// game task kept ticking while the menu pump was blocked -- so every save-flow deadline would have
/// expired under a browsing user, and the freeze is what saved the flow. `== 0` with a dialog
/// demonstrably open instead says the whole frame stalled with the pump.
pub static SAVE_PICKER_OS_TICKS_FROZEN: AtomicUsize = AtomicUsize::new(0);
/// OS common file dialogs opened this session.
pub static SAVE_PICKER_OS_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
/// OS dialogs that closed returning a path we accepted.
pub static SAVE_PICKER_OS_CLOSED_WITH_PATH: AtomicUsize = AtomicUsize::new(0);
/// OS dialogs the user cancelled (`FALSE` with `CommDlgExtendedError() == 0`).
pub static SAVE_PICKER_OS_CANCEL_COUNT: AtomicUsize = AtomicUsize::new(0);
/// OS dialogs comdlg32 failed (`FALSE` with a non-zero extended error), and the last such error.
/// Distinguished from a cancel because only a failure is a bug of ours, and neither reopens.
pub static SAVE_PICKER_OS_ERROR_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OS_LAST_ERROR: AtomicUsize = AtomicUsize::new(0);
/// Picks the shared save-validity predicate rejected, and the last `PickRejection as usize`.
pub static SAVE_PICKER_OS_REJECT_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OS_LAST_REJECT_REASON: AtomicUsize = AtomicUsize::new(0);
/// Dialog reopens after an invalid pick, and 1 if the bound was ever hit.
///
/// The bound is not about user patience: a comdlg32 that fails instantly (Wine's is a
/// reimplementation) would spin the reopen loop at full speed on the thread that owns the menu
/// pump, an unbreakable hang. Exhaustion takes the cancel path.
pub static SAVE_PICKER_OS_REOPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_OS_REOPEN_EXHAUSTED: AtomicUsize = AtomicUsize::new(0);
/// The `hwndOwner` handed to comdlg32 (0 = none found).
///
/// Which window this is changed on 2026-07-31 and the old expectation is now wrong. It used to be
/// required to be the game window; it is now the dim cover whenever a cover is up, because an owned
/// window is always above its owner and that is the only way to make "the picker is in front of the
/// blur" structural instead of a race. Read it together with `SAVE_PICKER_OS_OWNER_IS_COVER`:
/// `is_cover = 1` means this equals `SAVE_PICKER_DIM_HWND`, and `is_cover = 0` means it equals the
/// game window (the boot arm, which raises no cover, and the fallback when the cover did not come
/// up in time).
pub static SAVE_PICKER_OS_OWNER_HWND: AtomicUsize = AtomicUsize::new(0);
/// 1 when the last dialog was owned by the dim cover, 0 when it fell back to the ER window.
///
/// This is the field that says whether the z-order guarantee was actually in force for a given
/// open. A System>Quit open with `SAVE_PICKER_DIM_ARM_COUNT` advancing but `is_cover = 0` means the
/// cover was armed and the dialog still took the game window as its owner -- i.e. the cover did not
/// finish coming up inside `SAVE_PICKER_DIM_ARM_WAIT_MS` and the ordering is back to a race.
pub static SAVE_PICKER_OS_OWNER_IS_COVER: AtomicUsize = AtomicUsize::new(0);
/// Save-like `CreateFileW` opens observed while a dialog was open. Attribution for the shell
/// browsing traffic that otherwise pollutes the save CreateFileW diagnostics.
pub static SAVE_PICKER_OS_SAVELIKE_OPENS: AtomicUsize = AtomicUsize::new(0);

// ---- OS picker at the missing-save boot (startup_hooks/save_picker_boot.rs) ----
//
// The `SAVE_PICKER_OS_*` family above counts DIALOGS and is shared by all three intents. This
// family counts the boot intent's outcomes, which the shared family cannot express: at a
// missing-save boot a cancel is not "the user backed out of a menu", it quits the game, and that
// terminal step has to be provable from telemetry rather than from watching the screen.

/// Where the boot missing-save pick stands. `0` idle (nothing opened, or not a missing-save boot),
/// `1` a surface owns the pick, `2` a file was accepted and the character sub-picker owns it,
/// `3` the user cancelled the OS dialog and the game is quitting, `4` comdlg32 was unusable and the
/// in-game browser took the pick over.
pub static SAVE_PICKER_OS_BOOT_STATE: AtomicUsize = AtomicUsize::new(0);
/// Boot OS dialogs this session. At a missing-save boot exactly one open is ever started, so `> 1`
/// means the one-shot latch leaked and the reopen loop this design exists to prevent came back.
pub static SAVE_PICKER_OS_BOOT_OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Boot OS picks that cleared the shared validity predicate and reached the character sub-picker.
pub static SAVE_PICKER_OS_BOOT_PICK_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The acceptance oracle for the boot cancel path: the user pressed Cancel on the boot OS dialog
/// and the game is quitting.
///
/// Only trustworthy when `SAVE_PICKER_BOOT_TELEMETRY_FLUSHED` reads 1. When it reads 0 this field
/// is whatever it was before the cancel, and `er-quickload-bootstrap.jsonl`'s
/// `boot_picker_cancel_exit` record is the outcome instead.
pub static SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// 1 once the picker thread is calling `ExitProcess(0)`.
///
/// (An earlier `SAVE_PICKER_OS_BOOT_EXIT_PENDING` companion was removed with the game-task exit
/// hand-off it belonged to: the hand-off never executed at a missing-save boot, because the game
/// task had stopped ticking long before the user answered the dialog. The picker thread now does
/// the whole thing, so there is no interval during which an exit is owed but not performed.)
pub static SAVE_PICKER_OS_BOOT_EXIT_PERFORMED: AtomicUsize = AtomicUsize::new(0);
/// Times the boot OS surface gave up and handed the pick to the in-game browser (comdlg32 failed,
/// the reopen bound was exhausted, or the core `CreateFileW` detour never went live). Non-zero says
/// the user still got a picker, which is why this is a fallback and not a failure.
pub static SAVE_PICKER_OS_BOOT_FALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Ticks the boot OS open was deferred waiting for the core `CreateFileW` detour to go live. The
/// wait is bounded; on exhaustion the in-game browser takes over rather than the boot stranding.
pub static SAVE_PICKER_OS_BOOT_DEFER_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Did the picker thread manage to refresh the telemetry file before quitting?
///
/// `1` the file you are reading describes the cancel. `0` the flush could not run (the state mutex
/// was held by a thread that is not giving it back), so **every other field in this file predates
/// the cancel** and only `er-quickload-bootstrap.jsonl` plus the debug log describe the outcome.
///
/// This field exists because its absence cost a diagnosis. In run pr109-boot-oscancel-20260730-110704
/// the cancel worked perfectly and the telemetry showed `boot_state = OPEN`, `cancel_exit_count = 0`
/// -- identical to what a dialog that never returned would have written, because the file had gone
/// stale 12s earlier. A reader had no way to tell a working feature from a broken one.
pub static SAVE_PICKER_BOOT_TELEMETRY_FLUSHED: AtomicUsize = AtomicUsize::new(0);
/// `GAME_TASK_TICKS_TOTAL` sampled by the PICKER thread when the boot dialog opened, and again when
/// the user answered it. Both are written by a thread that is demonstrably alive, so their
/// difference is the direct answer to "was the game task running while the dialog was up" -- the
/// question the first live run left open and no existing field could settle.
///
/// Equal values mean the game task did not tick once across the dialog's entire life.
pub static SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER: AtomicUsize = AtomicUsize::new(0);

// ---- OS-picker dim overlay (save_picker_dim_overlay.rs) ----
//
// These are deliberately a new family rather than a reuse of `SAVE_PICKER_OVERLAY_*`. That older
// family belongs to the DLL-drawn startup picker (`gpu_readback/save_picker_overlay.rs`, the
// no-save-boot browser) and is live; borrowing its counters would make two unrelated surfaces
// indistinguishable in one telemetry field.
//
/// 1 while the dim overlay is armed (a blocking OS dialog is up and we are covering the game).
/// Cleared by the arming guard's `Drop`, so an unwind through the dialog cannot strand it.
pub static SAVE_PICKER_DIM_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Arms and disarms. They must end equal; `arm - disarm == 1` with the process alive is a stranded
/// fullscreen dim, which is worse than not having the feature at all.
pub static SAVE_PICKER_DIM_ARM_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_DIM_DISARM_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Frames pushed to the compositor via `UpdateLayeredWindow` while armed.
///
/// The core oracle. The game/menu thread is blocked inside comdlg32 for the dialog's whole life, so
/// the game logs nothing and presents nothing during that window. This counter advancing across the
/// same interval is the objective proof that the animation ticked on a thread we own -- something no
/// game-render-path overlay could produce.
pub static SAVE_PICKER_DIM_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// `SAVE_PICKER_DIM_FRAMES` sampled at the start of the current arm, so the disarm can subtract and
/// report this arm'S frames.
///
/// The counter above is process-cumulative and the disarm line used to print it raw, which read as
/// a per-arm figure and was not one: a four-open run logged 108/241/362/423 where the arms had
/// actually pushed 108/133/121/61. Every one of those lines overstated its own arm, and the last
/// overstated it by 7x. Snapshotting at arm and subtracting at disarm is what makes the line say
/// what it claims to say. Written by the arming thread before the generation bump, so the overlay
/// thread cannot have pushed a frame of the new arm yet.
pub static SAVE_PICKER_DIM_FRAMES_AT_ARM: AtomicUsize = AtomicUsize::new(0);
/// Wall-clock milliseconds of the last completed armed interval (arm -> disarm). Pairs with the
/// dialog's own `after=Nms` log line: the two must agree, or the dim did not bracket the call.
pub static SAVE_PICKER_DIM_ALIVE_MS: AtomicUsize = AtomicUsize::new(0);
/// Why the last disarm happened: 1 = the dialog returned (normal), 2 = arming failed and rolled
/// back, 3 = the overlay thread bailed out and hid the window itself.
pub static SAVE_PICKER_DIM_TEARDOWN_REASON: AtomicUsize = AtomicUsize::new(0);
/// Furthest overlay-thread init stage reached: 1 = thread, 2 = class, 3 = window, 4 = DIB,
/// 5 = render loop entered. Anything below 5 says where bring-up died.
pub static SAVE_PICKER_DIM_STAGE: AtomicUsize = AtomicUsize::new(0);
/// Our overlay window, and the ER window we sized/stacked against.
pub static SAVE_PICKER_DIM_HWND: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_DIM_GAME_HWND: AtomicUsize = AtomicUsize::new(0);
/// `UpdateLayeredWindow` calls that returned an error while armed.
pub static SAVE_PICKER_DIM_UPDATE_FAILS: AtomicUsize = AtomicUsize::new(0);
/// Z-order oracle, sampled while armed: the top-down z-order ordinal of our overlay, of the ER
/// window, and of the foreign foreground window (the OS dialog). `usize::MAX` = not found.
///
/// This is what settles the ordering requirement without a screenshot. The contract is
/// `foreign < self < game`: the dialog above us, us above the game. `self > game` means the dim is
/// behind the game and invisible; `self < foreign` means the dim is covering the dialog the user
/// has to interact with.
pub static SAVE_PICKER_DIM_Z_SELF: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_GAME: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_FOREIGN: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The foreground window seen while armed that is neither ours nor the game's -- i.e. comdlg32's.
/// 0 means no foreign foreground window was ever observed while the dim was up.
pub static SAVE_PICKER_DIM_FOREIGN_FG_HWND: AtomicUsize = AtomicUsize::new(0);
/// Frames whose sampled z-order violated the cover's contract while armed, counted separately for
/// the two ways it can break.
///
/// Counters, not last-sample snapshots, because `SAVE_PICKER_DIM_Z_*` only carry the most recent
/// frame -- and the most recent frame is the one taken as the dialog is already tearing down, which
/// is exactly when the ordering is least representative. `0` across a run where frames were pushed
/// is the real proof the stacking held for the whole dialog, not just at the end.
///
/// They are two fields because they mean opposite things and carry opposite SEVERITIES (split
/// 2026-08-01, er-effects-rs-mc1d). The fused predecessor `SAVE_PICKER_DIM_Z_VIOLATIONS` scored
/// `behind_game || covering_dialog` into one atomic, and the live run that was supposed to prove
/// the ownership fix came back with 130 of them across 424 dim frames -- a number from which
/// neither failure could be confirmed nor excluded. The oracle could not answer the single question
/// it was built to answer, so the run could neither pass nor fail the fix. The severities:
///
/// - `_Z_COVERING_DIALOG` (`self_z < foreign_z`, our cover nearer the front than the dialog) is
///   precisely the defect the ownership chain exists to eliminate. Non-zero means the fix is
///   incomplete and for those frames the user was looking at a dim laid over the controls they have
///   to click. Treat any non-zero value as a failure of the z-order fix.
/// - `_Z_BEHIND_GAME` (`self_z >= game_z`) is a lower-severity cosmetic failure: the cover is
///   invisible for those frames, but the dialog is still fully usable. Non-zero deserves its own
///   issue, not a block on the ownership work.
///
/// Unknown ordinals (`usize::MAX`) are excluded from both, so neither counts a window that had
/// merely dropped out of the z-chain while being created or destroyed.
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME: AtomicUsize = AtomicUsize::new(0);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// The first offending sample of each kind: the `(self, game, foreign)` ordinals that broke the
/// contract, plus the milliseconds between that arm's cover coming up and the break. `usize::MAX`
/// (emitted as `-1`) means that kind never fired.
///
/// A total alone cannot say *where in the arm* the break sat, and the phase is most of the
/// diagnosis: ordinals that break at `+0ms` and then settle are a bring-up transient the compositor
/// resolves, while the same ordinals still breaking hundreds of milliseconds in are a stacking that
/// never took. The run that motivated this recorded 130 breaks over 4 arms with no way to tell
/// those two apart.
///
/// First-wins, not last-wins, and enforced rather than assumed: the `_FIRST_SELF` field is the
/// whole record's claim ticket, taken by a `compare_exchange` off the `usize::MAX` sentinel, and
/// only the sample that wins that CAS writes the other three. A violating sample always has a known
/// `self_z` (both disjuncts require it), so the sentinel can never collide with a real value. The
/// first break is the one that shows the transition into failure; a last-wins record would decay
/// into the same tear-down-moment snapshot the counters above exist to avoid.
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_SELF: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_GAME: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_FOREIGN: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_MS: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_SELF: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_GAME: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_FOREIGN: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub static SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_MS: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Frames whose push had to fall back to a full-surface upload because the dirty-rectangle path was
/// refused. The cover is a mostly-static image with a small animating mark, so pushing only the
/// mark's rectangle is what keeps the pulse smooth; a run where this equals the frame count is a run
/// whose animation is paying a full-screen upload per frame (measured: ~9fps on a 3846x2172 window).
pub static SAVE_PICKER_DIM_FULL_PUSHES: AtomicUsize = AtomicUsize::new(0);
/// Result of the one bring-up push of `UpdateLayeredWindow`, done at attach on a hidden,
/// fully-transparent 1x1 layer: 0 = not attempted, 1 = accepted, 2 = rejected.
///
/// `UpdateLayeredWindow` is the single API in this feature that Wine could plausibly not implement
/// the way we need. Proving it at attach -- when nothing is waiting -- rather than at the instant a
/// user's dialog opens means a broken environment is visible in telemetry from a run that never even
/// opened a picker, instead of surfacing as a missing cover at the worst moment.
pub static SAVE_PICKER_DIM_SELFTEST: AtomicUsize = AtomicUsize::new(0);
// ---- Cover ownership + arm handshake (user report 2026-07-31) ----
//
// Two defects were reported against the same window: the OS picker came up behind the cover, and
// the cover could be dragged off the game as if it were an unrelated application. Both were the
// same root cause -- the cover was an UNOWNED top-level popup whose only claim to a z-order was one
// `HWND_TOP` raise, issued by the overlay thread up to a frame period after `arm` returned and
// therefore quite possibly after comdlg32 had already created its window. The fix makes both
// relations structural (game owns cover, cover owns dialog), and these fields are how a run proves
// the relations actually took rather than being assumed.
//
/// Did the cover get installed as an owned window of the ER window? 0 = never attempted (no game
/// window known), 1 = `SetWindowLongPtrW(GWLP_HWNDPARENT)` stored and the owner read back equal,
/// 2 = attempted and the read-back did not match, i.e. this environment ignored the store.
///
/// A read-back rather than the call's return value on purpose: `SetWindowLongPtrW` returns the
/// previous value, and 0 means both "there was no owner" and "the call failed", so its return
/// cannot distinguish success from failure on the very first store.
pub static SAVE_PICKER_DIM_OWNER_SET: AtomicUsize = AtomicUsize::new(0);
/// The owner HWND read back out of the cover's `GWLP_HWNDPARENT`. Equal to
/// `SAVE_PICKER_DIM_GAME_HWND` is the proof the attachment took; 0 with `_owner_set = 2` says the
/// store was silently dropped.
pub static SAVE_PICKER_DIM_OWNER_READBACK: AtomicUsize = AtomicUsize::new(0);
/// Milliseconds the arming thread waited for the overlay thread to report the cover up at the
/// game's geometry, on the last arm.
///
/// `arm` used to return immediately and the caller went straight into `GetOpenFileNameW`, so the
/// cover's raise and comdlg32's window creation were unordered. The arm now blocks on an atomic
/// handshake, which is what makes the ordering real; this field is its cost. Tens of milliseconds
/// is the expected value (one overlay frame plus the full-screen DIB fill).
pub static SAVE_PICKER_DIM_ARM_WAIT_MS: AtomicUsize = AtomicUsize::new(0);
/// Arms that hit the handshake deadline instead of the cover reporting ready. Non-zero means the
/// overlay thread is wedged or too slow, the dialog fell back to owning itself to the game window,
/// and the stacking for those opens is a race again -- not a silent degradation.
pub static SAVE_PICKER_DIM_ARM_WAIT_TIMEOUTS: AtomicUsize = AtomicUsize::new(0);
/// Frames on which the cover was found to have drifted off the ER window's rect and was snapped
/// back. Ownership is what a compositor is supposed to honour, but a Wayland compositor with a
/// move-modifier can still drag any toplevel; this counts the times something moved the cover and
/// we pulled it back, so "the blur is attached to the game" is measured rather than hoped for.
pub static SAVE_PICKER_DIM_REANCHOR_COUNT: AtomicUsize = AtomicUsize::new(0);
