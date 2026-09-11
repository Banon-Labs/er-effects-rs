//! The product's loading cover: how it arms and draws, how it lets go, and the vanilla
//! loading screen it is measured against.
//!
//! The `BOOT_VIEW_*` half is the cover's own lifecycle -- the window it is armed for, the
//! D3D12 objects it draws through, its one-way release fade, the release latches that
//! decide when letting go is safe, and the absolute backstop that ends a cover nothing
//! else released. The `NATIVE_LS_*` half is the counter-measurement: a Present frame on
//! which the game's own `CS::LoadingScreen` was live and the cover did not draw is the
//! frame a user sees vanilla, and each one is attributed to the `NATIVE_LS_GATE_*` code
//! for whatever blocked the composite.
//!
//! Split out of `counters.rs` as a pure code move: nothing is renamed and no initial value
//! changes. Every name here is re-exported from `er_telemetry_core::counters` with a glob, so
//! each consumer still spells it `er_telemetry_core::counters::<name>`.

use std::sync::atomic::{AtomicU64, AtomicUsize};

pub static BOOT_VIEW_DRAW_STATE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_STOPPED: AtomicUsize = AtomicUsize::new(0);
/// Why the boot-view cover last stopped (bd er-effects-rs-dpf6 Phase 1; 0 = armed/none,
/// 1 = release-fade after render-release, 2 = FPS bail, 3 = release-fade after can-move world proof).
/// Values are the `BOOT_VIEW_STOP_REASON_*` consts in boot_progress.rs. Reset to 0 on every rearm.
pub static BOOT_VIEW_STOP_REASON: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms when the current cover window was (re)armed (0 = initial boot window).
pub static BOOT_VIEW_WINDOW_ARM_MS: AtomicUsize = AtomicUsize::new(0);
/// Rearm -> stop duration (ms) of the last completed cover window (oracle_boot_view_cover_window_ms).
pub static BOOT_VIEW_COVER_WINDOW_MS_LAST: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_BG_PORTRAIT_RGBA_VERSION` snapshotted at the FPS-bail stop; a later version bump while the
/// native loading screen is still active is the Phase-2 resume trigger (bd er-effects-rs-dpf6).
pub static BOOT_VIEW_FPS_BAIL_PUBLISH_VERSION: AtomicUsize = AtomicUsize::new(0);
/// `BOOT_VIEW_OWN_MENU_LOAD_ACTIVE` slot key at the FPS-bail stop (the bail clears the live one; the
/// resume restores it so the own-menu stop semantics survive the resume).
pub static BOOT_VIEW_FPS_BAIL_SLOT_KEY: AtomicUsize = AtomicUsize::new(0);
/// Once-per-epoch resume latch: 1 after a publish-triggered FPS-bail resume in the current cover
/// window; also suppresses the permille re-bail for the rest of the window (the 20s composite cap
/// stays armed as the FPS backstop). Reset on every rearm.
pub static BOOT_VIEW_FPS_BAIL_RESUMED: AtomicUsize = AtomicUsize::new(0);
/// Cumulative count of publish-triggered FPS-bail resumes (oracle_boot_view_fps_bail_resumes).
pub static BOOT_VIEW_FPS_BAIL_RESUMES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_OWN_MENU_LOAD_ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LOADSCREEN_TABLE_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
/// `BOOT_VIEW_DRAW_HITS` as it stood when the cover window was last armed.
///
/// `BOOT_VIEW_DRAW_HITS` is cumulative for the life of the process and `boot_view_reset_cover_window`
/// deliberately does not clear it, so it cannot answer "has THIS cover epoch drawn anything". That
/// question is what `title_visual_suppression_active` needs: a rearmed cover that never composites
/// must not be allowed to force-hide the title. Subtracting this baseline turns the cumulative
/// counter into the per-epoch answer without disturbing any existing reader of the total.
pub static BOOT_VIEW_DRAW_HITS_ARM_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LAST_PERMILLE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DECISION_LOG_MS: AtomicU64 = AtomicU64::new(0);
pub static BOOT_VIEW_MONO_EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_MONO_ORD: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_MONO_LABEL_PTR: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_MONO_LABEL_LEN: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_REACHED_MASK: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_MILESTONE_IDX: AtomicUsize = AtomicUsize::new(0);
/// Load epoch identity (bd er-effects-rs-ok8d). One epoch = one arm-to-teardown lifetime of the bar.
/// `SEQ` increments on every epoch reset and is the key every per-epoch high-water latch is stamped
/// with, so a new epoch invalidates them all at once. It deliberately does not reuse the fresh-deser
/// counter, which only bumps at the reload's DESERIALIZE -- far too late to bound the epoch, and the
/// reason the visible label walked backwards mid-load.
pub static BOOT_VIEW_EPOCH_SEQ: AtomicUsize = AtomicUsize::new(0);
/// Which phase sequence this epoch publishes: 0 = process boot, 1 = character reload.
pub static BOOT_VIEW_EPOCH_KIND: AtomicUsize = AtomicUsize::new(0);
/// Per-epoch baselines for counters that are sticky for the whole process. A reload epoch must
/// assert its phases from what happened since the rearm, never from `!= 0` on a counter that a
/// previous load already moved (bd er-effects-rs-ok8d: load 2's mask opened at 0x9f because
/// `boot_milestone_reached` re-latched five boot phases from sticky counters the instant it ran).
pub static BOOT_VIEW_CONTINUE_ALLOW_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_TFC_CONTINUE_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PORTRAIT_SPARED_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FRESH_DESER_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LAST_LABEL_HASH: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_ALLOCATOR: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_LIST: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FENCE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_QUEUE: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_UPLOAD: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
pub static BOOT_VIEW_RTV_HEAP: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DRAW_BUSY: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_SELF_PRESENTS: AtomicUsize = AtomicUsize::new(0);
// Full-backbuffer black clears before copying the boot bar. `_PRESENT_` is the important product
// oracle: after the self-present pump yields to the game's render loop, every boot-view Present frame
// must still cover the rest of the game instead of leaving whatever Elden Ring rendered under/around
// the loading bar.
pub static BOOT_VIEW_SELF_FULL_CLEAR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PRESENT_FULL_CLEAR_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PRESENT_COVER_FAILURES: AtomicUsize = AtomicUsize::new(0);
// Nonzero means the cover stopped before a world/playable handoff. Native loading becoming visible is
// not enough; the product owns the full backbuffer until the game can safely show.
pub static BOOT_VIEW_PRE_WORLD_STOP_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_SWAPCHAIN_FOUND_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PUMP_STOP_REASON: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_PUMP_STOP_MS: AtomicU64 = AtomicU64::new(0);
pub static BOOT_VIEW_STRIP_W: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_STRIP_H: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DRAWN_PERMILLE: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_DRAWN_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_DRAWN_BG_ACTIVE: AtomicUsize = AtomicUsize::new(usize::MAX);
pub static BOOT_VIEW_HANDOFF_SEEN_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_START_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_COMPLETE_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_LAST_ALPHA: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_FADE_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_STOP_NATIVE_HITS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE: AtomicUsize = AtomicUsize::new(0);

// One-way release fade (user report 2026-08-22, second round: "I still see my portrait come back
// very briefly if I press escape too quickly after getting in game").
//
// The defect these measure. `native_gfx_hold_pending` in `composite_boot_progress_inner` is
// recomputed from scratch every frame out of two RECENCY predicates -- "the loading screen's
// Scaleform fade-out was stamped in the last 600 ms" and "CS::LoadingScreen::Update ticked in the
// last 900 ms". It was written as a START GATE for the release fade ("is it safe to begin fading
// yet?"), but nothing stopped it being re-asked once the fade was already running, and when it
// re-asserted the code fell through to the opaque cover path -- which, unlike the fade frame,
// rasterizes with `draw_portrait: true`. So a single fresh stamp mid-fade put the portrait back on
// screen at full alpha and then let the fade finish, which is exactly the "comes back very briefly
// and tears down" the user described.
//
// The stamp that did it was not the loading screen's. `scaleform_label_goto_hook` stamped
// `LOADING_SCREEN_GFX_FADEOUT_LAST_MS` on any timeline label merely containing "fadeout", on any
// movie, so opening the in-world menu was enough to refresh it. Narrowed 2026-09-05 to the loading
// screen's own fade clip; a foreign label now lands in `LOADING_SCREEN_GFX_FADEOUT_FOREIGN_HITS`.
/// `LOADING_SCREEN_UPDATE_HITS` snapshotted the frame the release fade started.
///
/// This is what tells a real hold from an over-matched one. Only the `CS::LoadingScreen::Update`
/// detour writes that counter, so a hold arriving mid-fade is backed by the game's own loading
/// screen if and only if the count has moved past this snapshot. A Scaleform label from some other
/// movie cannot move it. Per cover window.
pub static BOOT_VIEW_FADE_START_LS_UPDATE_HITS: AtomicUsize = AtomicUsize::new(0);
/// Frames the opaque cover path drew while this process's release fade was already running and had
/// not yet completed. The defect counter: it is the number that was 10 in the reproducing run
/// br-20260822-184123-fa3d (draws 528 -> 538 across the fade window) while every existing detector
/// read 0, because they were all gated on `BOOT_VIEW_STOPPED`, which the fade had not set yet.
///
/// Expected 0 for the life of the process now that the fade is one-way. Session-cumulative and
/// never cleared at a rearm, for the reason spelled out at `BOOT_VIEW_DRAW_AFTER_STOP_TOTAL`: a
/// detector a rearm can silently empty is not a detector.
pub static BOOT_VIEW_NONFADE_DRAW_DURING_FADE: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the first such frame (0 = never). Subtract `oracle_boot_view_fade_start_ms`
/// and the answer is how far into the fade the cover went opaque again.
pub static BOOT_VIEW_NONFADE_DRAW_DURING_FADE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Frames on which the start-gate predicate (`fadeout_pending || update_quiet_pending`) was true
/// while the release fade was already running -- i.e. every frame the old code would have taken
/// back to full opacity. Counted whether or not the hold was then honored, so the raw pressure on
/// the fade stays visible even when the new rule refuses all of it. Session-cumulative.
pub static BOOT_VIEW_FADE_HOLD_REASSERTS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the first re-assert (0 = none).
pub static BOOT_VIEW_FADE_HOLD_REASSERTS_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Re-asserts REFUSED: no `CS::LoadingScreen::Update` tick past
/// `BOOT_VIEW_FADE_START_LS_UPDATE_HITS` backed them, so they were an over-matched Scaleform label
/// and the fade carried on. This is the fix engaging; a nonzero value with
/// `BOOT_VIEW_NONFADE_DRAW_DURING_FADE == 0` is the defect being caught and refused.
pub static BOOT_VIEW_FADE_HOLD_REFUSED: AtomicUsize = AtomicUsize::new(0);
/// Re-asserts HONORED: the game's own loading screen really did tick again mid-fade, so the fade
/// paused at its current alpha rather than completing over live loading art (er-effects-rs-wmw
/// defect #1, the vanilla flash-through, is what that pause protects).
pub static BOOT_VIEW_FADE_HOLD_HONORED: AtomicUsize = AtomicUsize::new(0);
/// Total ms this cover window's release fade spent paused by honored holds. The fade clock
/// subtracts it, so the visible fade is always the full `BOOT_VIEW_RELEASE_FADE_MS` of ramp however
/// often it was interrupted. Uncapped on purpose: the cap is applied where it is used, so this
/// stays the honest measure of how long the game held us.
pub static BOOT_VIEW_FADE_HELD_MS: AtomicUsize = AtomicUsize::new(0);
/// Pause accumulator state, not an answer: the boot-view ms of the previous paused frame, or 0 when
/// the last frame was not paused. Differencing it is what turns per-frame pauses into `_HELD_MS`.
pub static BOOT_VIEW_FADE_HOLD_TICK_MS: AtomicUsize = AtomicUsize::new(0);
/// Consecutive re-asserting frames right now; 0 on any fade frame with no re-assert. Only the
/// frame that takes it from 0 to 1 logs, so one Escape press produces one line instead of the ~36
/// a 600 ms recency window would otherwise emit from inside Present.
pub static BOOT_VIEW_FADE_HOLD_REASSERT_RUN: AtomicUsize = AtomicUsize::new(0);
// Loud gap oracle: nonzero means the boot cover stopped from the bail clock before
// the native loading screen produced enough update ticks to be visibly lit.
pub static BOOT_VIEW_DARK_GAP_FAILURES: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DARK_GAP_LAST_HELD_MS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS: AtomicUsize = AtomicUsize::new(0);
// Handoff stamps made from telemetry/update context because the draw path may already be yielded or
// skipped on the exact frame the native loading screen first appears.
pub static BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_VIEW_IDX_CHANGED_MS: AtomicU64 = AtomicU64::new(0);
// Native loading-screen exposure (er-effects-rs-wmw defect #1: "the custom loading screen
// disappeared for about one frame and the vanilla loading screen flashed through"). One Present
// frame is an exposure frame when the game's own CS::LoadingScreen is live but our cover did not
// draw over the backbuffer -- exactly the frame the user sees vanilla. Counted in the Present
// detour, attributed to the gate that blocked the composite (`NATIVE_LS_GATE_*`).
/// Present frames with the native loading screen live and our cover not drawn.
///
/// Not the defect count on its own -- read [`NATIVE_LS_EXPOSURE_OWNED_FRAMES`] for that. This stays
/// the total of every such frame so no information is lost, but a loading screen the product does
/// not cover (fast travel, death, area transition) lands in it too; see
/// [`cover_owns_current_loading_screen`] for the split and the run that forced it.
pub static NATIVE_LS_EXPOSURE_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Present frames with the native loading screen live and our cover drawn (the healthy case).
pub static NATIVE_LS_COVERED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Consecutive exposure frames right now; resets on any covered frame.
pub static NATIVE_LS_EXPOSURE_CUR_RUN: AtomicUsize = AtomicUsize::new(0);
/// Longest consecutive exposure run this session. 1 == the user's "about one frame" flash.
pub static NATIVE_LS_EXPOSURE_MAX_RUN: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_LS_EXPOSURE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
pub static NATIVE_LS_EXPOSURE_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// `NATIVE_LS_GATE_*` code for the most recent exposure frame.
pub static NATIVE_LS_EXPOSURE_LAST_GATE: AtomicUsize = AtomicUsize::new(0);
/// `BOOT_VIEW_STOP_REASON` sampled at the most recent exposure frame.
pub static NATIVE_LS_EXPOSURE_LAST_STOP_REASON: AtomicUsize = AtomicUsize::new(0);
/// Per-gate exposure tallies, indexed by the `NATIVE_LS_GATE_*` codes.
pub static NATIVE_LS_EXPOSURE_BY_GATE: [AtomicUsize; NATIVE_LS_GATE_COUNT] =
    [const { AtomicUsize::new(0) }; NATIVE_LS_GATE_COUNT];

// Cover-after-release SEMAPHORES (user report 2026-08-22: "pressing Escape quickly after the
// loading screen fades out -- while the location banner is still on screen -- makes the loading
// screen and portrait briefly reappear and tear down").
//
// The run that reproduced it (br-20260822-184123-fa3d) left zero trace in the DLL log, and that
// invisibility is what this group exists to end. Every per-frame oracle we had switches itself off
// in exactly the window where the defect happens: `native_ls_exposure_record` early-returns unless
// the game's `CS::LoadingScreen` ticked within 250 ms, and in that run the native screen stopped
// ticking ~2.2 s before our cover stopped. So the one moment worth watching was the one moment
// nothing was watching.
//
// Two independent questions, deliberately kept apart:
//   1. Did our compositor draw after it latched stopped?  -> `BOOT_VIEW_DRAW_AFTER_STOP*`.
//      Expected 0 forever. It is a NULL DETECTOR: the whole diagnosis rests on the claim that our
//      cover did not draw the thing the user saw, and this is the counter that can refute it.
//   2. Was the game's own cover plate up, and was its loading screen still working?
//      -> `COVER_PLATE_*_AFTER_RELEASE` / `NATIVE_LS_ACTIVITY_AFTER_RELEASE_*`.
/// Frames on which the boot-view compositor incremented a draw/fade counter while
/// `BOOT_VIEW_STOPPED` was already set. Per cover window (cleared at every rearm).
///
/// Gate on this being NONZERO -- it is the check firing when it should not (bd k979). A nonzero
/// value means the 2026-08-22 diagnosis is wrong and our own compositor is drawing after release.
pub static BOOT_VIEW_DRAW_AFTER_STOP: AtomicUsize = AtomicUsize::new(0);
/// Session-cumulative twin of `BOOT_VIEW_DRAW_AFTER_STOP`, never cleared. The per-window counter
/// is the one to read when asking about the current cover window, but a rearm zeroes it, and a
/// detector that a rearm can silently empty is not a detector. This is the copy that remembers.
pub static BOOT_VIEW_DRAW_AFTER_STOP_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the first post-stop draw in the current cover window (0 = none).
pub static BOOT_VIEW_DRAW_AFTER_STOP_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms at which the current cover window latched `BOOT_VIEW_STOPPED` (0 = armed).
///
/// Deliberately not `BOOT_VIEW_FADE_COMPLETE_MS`, which the FPS-bail exit never sets. Written at
/// both stop sites, cleared at rearm and by the FPS-bail resume, so it always describes the live
/// latch rather than the last release fade.
pub static BOOT_VIEW_STOP_MS: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_SCREEN_UPDATE_HITS` snapshotted at the stop, so post-release native ticks are a delta.
pub static BOOT_VIEW_STOP_LS_UPDATE_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// `SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT` snapshotted at the stop, so a world load started
/// after the cover let go is a delta rather than a guess. Read by
/// [`cover_owns_current_loading_screen`], which is the whole reason it exists.
pub static BOOT_VIEW_STOP_LOAD_WITNESS: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_SCREEN_GFX_FADEOUT_HITS` snapshotted at the stop, for the same reason.
pub static BOOT_VIEW_STOP_LS_FADEOUT_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Present frames the post-release watch actually sampled. 0 means the watch never opened, which
/// is not the same answer as "sampled and saw nothing" -- without it every zero below is ambiguous.
pub static COVER_PLATE_AFTER_RELEASE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
/// Sampled frames where the game's own `CSFakeLoadingScreenImp` cover plate read visible after our
/// cover had already released. This is the decisive one: it says whether the surface the user
/// reported was the game's plate, on a frame that actually reached Present.
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE: AtomicUsize = AtomicUsize::new(0);
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// Consecutive visible-plate frames right now; resets on any sampled frame with the plate down.
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_CUR_RUN: AtomicUsize = AtomicUsize::new(0);
/// Longest consecutive visible-plate run since release. A brief reappearance is a short run; a
/// plate that simply never went down is one run as long as the watch.
pub static COVER_PLATE_VISIBLE_AFTER_RELEASE_MAX_RUN: AtomicUsize = AtomicUsize::new(0);
/// Largest `LOADING_SCREEN_UPDATE_HITS` delta observed past the stop baseline: the game's own
/// loading screen still ticking after our cover let go.
pub static NATIVE_LS_ACTIVITY_AFTER_RELEASE_UPDATES: AtomicUsize = AtomicUsize::new(0);
/// Same for `LOADING_SCREEN_GFX_FADEOUT_HITS` (Scaleform fade-out stamps past the stop).
pub static NATIVE_LS_ACTIVITY_AFTER_RELEASE_FADEOUTS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms the first post-release native activity was observed (0 = none).
pub static NATIVE_LS_ACTIVITY_AFTER_RELEASE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);

// In-game menu open stamp (2026-08-22). The post-release cover watch above can say a cover plate
// came back at ms X, and the user's report says the trigger is pressing Escape quickly after a
// load. Nothing in telemetry stamped the Escape press, so X could only be tied to the press by
// hand -- on a defect whose entire signature is the interval between the two. The three oracles
// below (plus one internal edge-detector state) make that interval a measured number.
//
// The signal. Not a new hook and not a state poll: the game's own `02_000_IngameTop`
// `MenuWindowJob` running. The product `MenuWindowJob::Run` detour (the PAB one, the deterministic
// winner at 0x7ad1c0) already dispatches on that wide resource name to maintain
// `SYSTEM_QUIT_INGAME_TOP_WINDOW`, and `02_000_IngameTop` is the game's own name for the in-world
// pause/System menu -- the window Escape opens. The job runs once per frame while that menu is up
// and not at all otherwise, so a tick after a gap is the open. Ground truth for both halves of
// that claim, from the shipped DLL's own log of a real session (2026-08-22 11:41:30 run, game-dir
// `er-quickload-autoload-debug.log`): zero `02_000_IngameTop` `MenuWindowJob::Run` lines through the
// first 39.9 s of boot, character load and gameplay, then a first line at `[+39905ms]` carrying
// `prev=0x0`, followed by lines every ~20-60 ms -- one per presented frame -- while the menu was
// open.
//
// What the edge costs in precision. "After a gap" needs a threshold
// (`IN_GAME_MENU_TICK_GAP_MS`), so two menu sessions closer together than that read as one, and
// if the job ever pauses while a SUBMENU owns the screen, coming Back reads as a second open. Both
// errors are in `_EDGES`; the latch below takes only the first edge past a cover stop, which is
// the press being asked about.
/// Boot-view-epoch ms of the most recent `02_000_IngameTop` `MenuWindowJob::Run` tick (0 = never).
/// This is the edge detector's own state, not an answer: it is compared against the next tick to
/// decide whether that tick continues a menu session or starts one.
pub static IN_GAME_MENU_RUN_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// Times the in-game menu was observed opening (a tick more than `IN_GAME_MENU_TICK_GAP_MS` after
/// the previous one, or the first ever). Session-cumulative.
pub static IN_GAME_MENU_OPEN_EDGES: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the most recent open edge (0 = the menu never opened this session).
pub static IN_GAME_MENU_OPEN_LAST_MS: AtomicUsize = AtomicUsize::new(0);
/// Boot-view-epoch ms of the first open edge that landed while `BOOT_VIEW_STOPPED` was set -- i.e.
/// the first time the user opened the menu after a cover released. That is the press the
/// 2026-08-22 report describes, so `COVER_PLATE_VISIBLE_AFTER_RELEASE_FIRST_MS` minus this is the
/// press-to-reappearance interval, on one clock, with no hand derivation. 0 = no such open.
///
/// Process-lifetime, deliberately: the counter it is meant to be subtracted from has exactly the
/// same lifetime (also a compare-exchange-from-0 latch that no rearm clears), so on a run with
/// several loads both describe the first occurrence and the subtraction stays meaningful. Give this
/// one a per-window reset and the pair would silently start describing different windows.
pub static IN_GAME_MENU_OPEN_FIRST_MS_AFTER_COVER_STOP: AtomicUsize = AtomicUsize::new(0);
/// How many of the session's earliest open edges are stamped individually.
///
/// Eight, because the reported defect is driven by the first press after a load and the loads that
/// matter come a handful of menu sessions into a run; carrying more would cost atomics in a Present
/// path for edges nobody reads.
pub const IN_GAME_MENU_OPEN_MS_FIRST_N_LEN: usize = 8;
/// Boot-view-epoch ms of the first open edge of the session, whenever it happened (0 = never).
///
/// Why it exists. The 2026-08-22 round shipped only `_LAST_MS` and
/// `_FIRST_MS_AFTER_COVER_STOP`, and both missed the press being investigated: in run
/// br-20260822-184123-fa3d the user's fast Escape landed during the release fade, before
/// `BOOT_VIEW_STOPPED` latched, so the after-stop latch recorded the later control press (36537 ms)
/// and edge #1's timestamp was simply not recoverable from telemetry at all. A press that precedes
/// the stop is precisely the press the report is about, so it must be in the record.
pub static IN_GAME_MENU_OPEN_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// The first `IN_GAME_MENU_OPEN_MS_FIRST_N_LEN` open edges, in order, each at its own index.
///
/// Edges past the end are counted by `IN_GAME_MENU_OPEN_EDGES` and stamped by `_LAST_MS`; they are
/// not individually recoverable, which is the deliberate cost of a fixed-size array in a per-frame
/// path. Zero at an index below `_EDGES` means that edge fired before the boot-view clock was
/// anchored, not that it did not happen.
pub static IN_GAME_MENU_OPEN_MS_FIRST_N: [AtomicUsize; IN_GAME_MENU_OPEN_MS_FIRST_N_LEN] =
    [const { AtomicUsize::new(0) }; IN_GAME_MENU_OPEN_MS_FIRST_N_LEN];

// The two clocks (2026-08-22). The DLL debug log's `[+Nms]` prefix and every telemetry `*_ms`
// field are measured from different epochs, both lazily anchored `Instant`s: the log's
// `PROCESS_LOG_EPOCH` starts at the first log line (near DLL_PROCESS_ATTACH) and the telemetry
// clock's `BOOT_VIEW_EPOCH` starts at the first `boot_view_epoch_ms()` call (once the boot view
// runs, seconds later). The gap between them is a constant for the whole process, but it had to be
// measured by pairing a log line against the oracle it wrote -- by hand, once per session, from
// scratch every time. Measured this way on the 2026-08-22 run: log `+40554` == `fade_complete_ms
// 37714`, log `+35735` == `release_ready_ms 32895`, log `+39281` == `fade_start_ms 36440` -- 2840,
// 2840, 2841. The DLL knows both numbers; it can simply say so.
/// `log_ms - telemetry_ms`: Add this to any telemetry `*_ms` to get the `[+Nms]` log prefix it
/// corresponds to, subtract it from a log prefix to get the telemetry clock. Measured once, from
/// both clocks read back to back. 0 = not measured yet (the boot-view clock had not started).
pub static LOG_EPOCH_OFFSET_MS: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch for the clock-map log line, so it is stated once per process and not per frame.
pub static LOG_EPOCH_OFFSET_LOGGED: AtomicUsize = AtomicUsize::new(0);

// Cover release LATCHES (er-effects-rs-drb7). The cover's release needs the player to be
// render-ready and the native loading screen to be finishing. Both happen in a normal session but
// not at the same instant (measured: render-ready at +27491ms, native close much later), and the
// predicate required them simultaneously, so it never fired in product. Latch each per cover
// window; both latched = release. Cleared by `boot_view_reset_cover_window`.
/// Set once the local player has been observed render-enabled during this cover window.
pub static BOOT_VIEW_RELEASE_RENDER_READY_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Set once the native loading screen has been observed closing/complete during this cover window.
pub static BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Epoch ms at which both latches were first satisfied (the real handoff instant); 0 = not yet.
pub static BOOT_VIEW_RELEASE_READY_MS: AtomicUsize = AtomicUsize::new(0);
/// Cover windows released by the real end condition rather than by a bail. The product-health
/// counter: on a healthy session this should equal the number of character loads (boot + N
/// switches) -- Not the number of native loading screens, of which a switch shows two.
pub static BOOT_VIEW_SEMANTIC_RELEASES: AtomicUsize = AtomicUsize::new(0);

// Portrait reject attribution (er-effects-rs-k979). `LS_PORTRAIT_REJECTED_PUBLISHES` is a bare
// count with no reason and no ordering, so a proof could only ask "were there any rejects", and
// answering yes failed the run. But refusing a blank frame is the neutral gate WORKING: measured in
// run slot-portrait-proof-20260731-130803, the neutral leak was first seen at capture version 1 --
// the very first capture -- 2 frames were refused out of 1542, and all 1540 publishes were clean.
// That is warm-up, not a defect. What would be a defect is a refusal after the window has published
// cleanly: the pipeline started emitting blanks mid-window. These let the two be told apart.
/// Capture version stamped at the most recent rejected publish. Compared against a window's publish
/// baseline to place the reject before or after that window's first clean publish.
pub static LS_PORTRAIT_REJECT_LAST_VERSION: AtomicUsize = AtomicUsize::new(0);
/// Neutral percentage of the most recent rejected frame. `LS_PORTRAIT_LAST_NEUTRAL_PCT` is
/// overwritten by every capture, so the value that actually caused the refusal was being lost.
pub static LS_PORTRAIT_REJECT_LAST_NEUTRAL_PCT: AtomicUsize = AtomicUsize::new(0);
/// `LOADING_BG_PORTRAIT_RGBA_VERSION` snapshotted when the current portrait window opened. The
/// version counter is cumulative for the whole process, so "has anything published yet" is only
/// answerable against this baseline -- comparing against 0 would misfile every warm-up reject from
/// the second window onward as a post-publish fault.
pub static LS_PORTRAIT_REJECT_PUBLISH_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Rejects that occurred before this window published a clean frame (pipeline warm-up).
pub static LS_PORTRAIT_REJECTS_BEFORE_WINDOW_PUBLISH: AtomicUsize = AtomicUsize::new(0);
/// Rejects that occurred after this window published cleanly -- the signal worth failing a proof
/// on: the pipeline began emitting blanks mid-window.
pub static LS_PORTRAIT_REJECTS_AFTER_WINDOW_PUBLISH: AtomicUsize = AtomicUsize::new(0);

// Character-load release gate (er-effects-rs-q6vk). A profile switch presents two native loading
// screens: the return-to-title teardown, then the character load after continue_confirm. Both
// satisfy "player render-ready + native screen finishing", so the cover released on the first and
// left the character load bare. These hold the release until this switch's character load has
// actually begun, identified by the fresh-deser count advancing past its value at arm time.
/// `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT` snapshotted when the cover armed for a switch.
pub static BOOT_VIEW_RELEASE_CONFIRM_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// 1 while the current cover window must wait for a character load (set at an own-menu switch arm,
/// cleared for boot, whose single load has no teardown screen in front of it).
pub static BOOT_VIEW_RELEASE_REQUIRE_CONFIRM: AtomicUsize = AtomicUsize::new(0);
/// Times the gate held a release that would otherwise have fired on the teardown screen. Proves the
/// gate engaged; 0 on a chain with switches means it is not doing anything.
pub static BOOT_VIEW_RELEASE_HELD_FOR_CONFIRM: AtomicUsize = AtomicUsize::new(0);
/// Releases that still landed before their switch's character load began. Must stay 0.
pub static BOOT_VIEW_RELEASE_BEFORE_CONFIRM: AtomicUsize = AtomicUsize::new(0);

// Absolute cover BACKSTOP (user report 2026-08-30). A session spent 7+ minutes with the loading
// cover full-clearing the backbuffer over live gameplay, with no way out short of killing the
// process: the proximate cause was a game-image detour that failed to install, and both of the
// cover's exits are downstream of that same detour (see `boot_view_absolute_backstop`). These
// three count/describe the last-resort release that now exists for that case.
//
// Every one of these is a defect report, not a success. `BOOT_VIEW_BACKSTOP_RELEASES` is expected
// to be 0 for the life of a healthy process; a run that reports any is a run to investigate, and
// `_TRIGGER` says which of the two arms fired so the investigation starts in the right place.
// Deliberately not cleared by `boot_view_reset_cover_window`: a detector a rearm can silently
// empty is not a detector (same argument as `BOOT_VIEW_DRAW_AFTER_STOP_TOTAL`).
/// Cover windows released by the absolute backstop rather than by any healthy exit. Must stay 0.
pub static BOOT_VIEW_BACKSTOP_RELEASES: AtomicUsize = AtomicUsize::new(0);
/// Boot-view epoch ms at which the backstop first tripped in this process; 0 = never.
pub static BOOT_VIEW_BACKSTOP_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
/// Which arm tripped most recently: 0 = never, 1 = world demonstrably live under an opaque cover,
/// 2 = wall-clock cover lifetime exceeded (see `BOOT_VIEW_BACKSTOP_TRIGGER_*`).
pub static BOOT_VIEW_BACKSTOP_TRIGGER: AtomicUsize = AtomicUsize::new(0);

/// The cover drew this frame -- not an exposure.
pub const NATIVE_LS_GATE_DREW: usize = 0;
/// `portrait_overlay_enabled()` was false, so the composite was never attempted.
pub const NATIVE_LS_GATE_OVERLAY_DISABLED: usize = 1;
/// The in-world epoch fast-path skip fired (world clock live for the current load epoch).
pub const NATIVE_LS_GATE_EPOCH_WORLD_LIVE: usize = 2;
/// The native-Windows pre-loading-screen composite suppression fired.
pub const NATIVE_LS_GATE_NATIVE_SUPPRESSED: usize = 3;
/// The composite ran but drew nothing (internally gated: `BOOT_VIEW_STOPPED` / draw-state).
pub const NATIVE_LS_GATE_COVER_STOPPED: usize = 4;
/// The composite drew nothing because the product does not cover this loading screen -- see
/// [`cover_owns_current_loading_screen`]. Expected, not a defect: the cover is a boot/character-load
/// surface, and a fast travel, a death respawn or an area transition is the game's own screen.
pub const NATIVE_LS_GATE_UNOWNED_LOAD: usize = 5;
pub const NATIVE_LS_GATE_COUNT: usize = 6;

/// Present frames the cover should have covered and did not: [`NATIVE_LS_EXPOSURE_FRAMES`] minus
/// the [`NATIVE_LS_GATE_UNOWNED_LOAD`] frames. This is the number an acceptance gate reads.
pub static NATIVE_LS_EXPOSURE_OWNED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Boot-view epoch ms of the first owned exposure frame (0 = none).
pub static NATIVE_LS_EXPOSURE_OWNED_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
