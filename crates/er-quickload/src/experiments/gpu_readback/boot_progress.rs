use super::*;
use er_game_base::fnv1a::fnv1a64;
// Boot-progress view -- our own pre-Continue cover content, drawn from the FIRST presented frame.
//
// With the splash/logo/title visuals suppressed, every frame the game presents between its first
// `Present` (~+3.5s after attach) and the post-Continue loading window (~+15.5s) is pure black. The
// Present-hook VMT swap is already installed BEFORE the first present (task tick ~+3.0s), so the
// black gap is a draw-gating matter, not a hook-timing one: this module opens the gate at Present
// hit #1 with content that needs NOTHING from the game -- a hairline loading bar in the game's own
// understated presentation plus a small milestone label (5x7 embedded font, procedurally
// rasterized, no game-derived assets), progress driven purely by our own already-latched RAM
// semaphores:
//
//   BOOT     -- drawing at all (present hook + swapchain live)
//   GAME     -- `game_man_ptr_or_null() != 0` (GameMan constructed)
//   OFFLINE  -- `FORCE_OFFLINE_BYTES_CLEARED` (GameMan online bytes cleared, ~+8.5s)
//   TITLE    -- `TITLE_FADEIN_SKIP_FIRED` (zero-input FadeIn->Loop transition)
//   MENU     -- `PRODUCT_CORE_LAST_MENU_OPENED_LATCH` (title menu natural-open latch)
//   CONTINUE -- `SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT` / `TFC_CONTINUE_FIRED`
//               (the visible SAVE LOAD tick marks the save-data pause immediately after this)
//   LOADING  -- forced Continue/native CS::LoadingScreen update -> HANDOFF. Early profile/keyed
//               semaphores only keep the cover drawing; they do not start the bail clock.
//
// Reached milestones are latched into a monotonic bitmask (a latch that later reads 0 cannot walk
// the bar backwards), and the displayed value creeps part-way toward the next milestone over time so
// the bar visibly moves between semaphores. The draw is a single submit on our OWN queue (transition
// PRESENT->COPY_DEST, CopyTextureRegion upload->backbuffer strip rect, transition back, CPU fence
// wait) -- no backbuffer readback: the pre-Continue frames are the content-free black this view
// exists to replace, and the strip rect is entirely ours.

/// Cover-window measurability + FPS-bail resume state (bd er-effects-rs-dpf6 Phases 1+2): why the
/// cover last stopped, when the window armed, the last window's arm->stop duration, and the
/// publish-version/slot-key snapshots + once-per-epoch latch behind the publish-triggered resume.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_COVER_WINDOW_MS_LAST;
/// DIAGNOSTIC (bd ab-portrait-disabled-load2-fps-still-low-boot-view-composite-is-killer-2026-07-20):
/// last process-ms the per-frame boot-view stop DECISION was logged, so the log is rate-limited to
/// ~1/s while we diagnose why neither stop path fires for the incomplete load2.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DECISION_LOG_MS;
/// Per-frame composite counter (RAM semaphore: the boot view is actually reaching the backbuffer).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DRAW_HITS;
/// Draw-state machine: 0 = uninit, 1 = ready, 2 = failed (give up; never retry).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DRAW_STATE;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FPS_BAIL_PUBLISH_VERSION;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FPS_BAIL_RESUMED;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FPS_BAIL_RESUMES;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FPS_BAIL_SLOT_KEY;
/// Last DISPLAYED progress in permille (monotonic; includes the inter-milestone creep).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_LAST_PERMILLE;
/// Baseline `PROFILE_LOADSCREEN_TABLE_BUILDS` when the own-menu switch rearmed the boot view; a later
/// increment is this switch's loading-window handoff. Default 0 preserves first-start behavior.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_LOADSCREEN_TABLE_BASELINE;
/// Monotonic-display clamp for the (phase idx, substep) LABEL numbers (user 2026-07-19): the visible
/// numbers must only advance within one load epoch -- never repeat a value already passed nor
/// decrement -- or the loading text reads as jumpy/looping. Ordinal = idx*ORD_SCALE + sub (phase idx
/// dominates). Held label is a `&'static str` (all sub-labels come from const tables, so its ptr/len
/// stay valid forever). Reset per load epoch.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_MONO_EPOCH;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_MONO_LABEL_LEN;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_MONO_LABEL_PTR;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_MONO_ORD;
/// Nonzero while the System->Quit custom ProfileSelect flow is switching to a picked slot. Value is
/// selected_slot + 1 so slot 0 is representable. This reopens the boot bar after the first world load.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_OWN_MENU_LOAD_ACTIVE;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_STOP_REASON;
/// One-shot stop latch: the loading window / world took over; reset only for a deliberate own-menu
/// character switch so the same custom progress bar can cover the return-title/autoload black gap.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_STOPPED;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_WINDOW_ARM_MS;
/// `BOOT_VIEW_STOP_REASON` values (0 = armed/none).
pub(crate) const BOOT_VIEW_STOP_REASON_RELEASE_FADE: usize = 1;
pub(crate) const BOOT_VIEW_STOP_REASON_FPS_BAIL: usize = 2;
pub(crate) const BOOT_VIEW_STOP_REASON_WORLD_HANDOFF: usize = 3;
/// The absolute backstop ended the window. Reasons 1-3 are the cover working; this one is the
/// cover having FAILED and being torn down by force. Kept distinct so no reader can file a
/// backstop release as a healthy one -- see [`boot_view_absolute_backstop`].
pub(crate) const BOOT_VIEW_STOP_REASON_ABSOLUTE_BACKSTOP: usize = 4;
const BOOT_VIEW_MONO_ORD_SCALE: usize = 1000;
/// Load-epoch identity + the per-epoch baselines for process-sticky counters (bd er-effects-rs-ok8d).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_CONTINUE_ALLOW_BASELINE;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_EPOCH_KIND;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_EPOCH_SEQ;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FRESH_DESER_BASELINE;
/// Hash of the last composed visible loading label logged to the runtime debug log.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_LAST_LABEL_HASH;
/// Highest reached milestone index (drives the label).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_MILESTONE_IDX;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_PORTRAIT_SPARED_BASELINE;
/// Monotonic bitmask of reached milestones (bit i = milestone i seen reached at least once).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_REACHED_MASK;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_TFC_CONTINUE_BASELINE;
/// `BOOT_VIEW_EPOCH_KIND` values.
const BOOT_VIEW_EPOCH_KIND_RELOAD: usize = 1;
use crate::constants::BOOT_VIEW_EPOCH_COMPOSITE_CAP_MS as COMPOSITE_CAP;
use er_loading_portrait_core::native_loading_progress as np;

/// LOADING_SCREEN_UPDATE_HITS baseline latched at handoff detection: the counter is cumulative
/// across loads, so an own-menu second load must measure only ITS loading screen's ticks.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DARK_GAP_FAILURES;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DARK_GAP_LAST_HELD_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS;
/// Draw mutual-exclusion latch: the self-present pump thread and the game's render thread (Present
/// detour) share the command allocator/list; whoever loses the swap skips its frame.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DRAW_BUSY;
/// 1 when the last rasterized upload included the optional cached screenshot background.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DRAWN_BG_ACTIVE;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DRAWN_IDX;
/// Last (permille, idx) actually rasterized into the upload buffer (skip the map/write when unchanged).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_DRAWN_PERMILLE;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_COMPLETE_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_FAILURES;
/// One-way release fade (2026-08-22): how long the fade was PAUSED by an honored hold, the pause
/// accumulator's own previous-frame stamp, and the re-assert tallies the pause is decided from.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HELD_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HITS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HOLD_HONORED;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HOLD_REASSERT_RUN;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HOLD_REASSERTS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HOLD_REASSERTS_FIRST_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HOLD_REFUSED;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_HOLD_TICK_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_LAST_ALPHA;
/// `LOADING_SCREEN_UPDATE_HITS` as it stood when this window's release fade began -- the snapshot a
/// mid-fade hold is tested against, so an over-matched Scaleform label cannot pass for the game's
/// own loading screen still running.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_START_LS_UPDATE_HITS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_FADE_START_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE;
/// Epoch-ms (never 0 once set) when the real loading/world handoff was first detected; the hold
/// clock for the seamless cut. Early profile/keyed-frame semaphores must not start this clock.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_HANDOFF_SEEN_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS;
/// THE defect counter for the 2026-08-22 "portrait comes back if I press escape too quickly"
/// report: opaque cover draws that happened while the release fade was already running. Expected 0.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_NONFADE_DRAW_DURING_FADE;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_NONFADE_DRAW_DURING_FADE_FIRST_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_PRE_WORLD_STOP_FAILURES;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_PRESENT_COVER_FAILURES;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_PRESENT_FULL_CLEAR_HITS;
/// Why the self-present pump stopped: 0 = still running/never ran, 1 = game started presenting
/// (the goal), 2 = timeout budget, 3 = Present returned a failure HRESULT.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_PUMP_STOP_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_PUMP_STOP_REASON;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_SELF_FULL_CLEAR_HITS;
/// Frames WE presented on the game's swapchain before its render loop produced its first frame.
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_SELF_PRESENTS;
/// CS::LoadingScreen update hits at the moment the cover stopped (telemetry: proves the cut
/// happened on a lit loading screen, not into the black gap).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_STOP_NATIVE_HITS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_STRIP_H;
/// (w, h) the current upload buffer was rasterized for (strip geometry follows the backbuffer).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_STRIP_W;
/// Pump-relative ms at which the game swapchain was found + hooked (0 = never; pump path only).
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_SWAPCHAIN_FOUND_MS;
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS;
/// Creep timing epoch + the epoch-ms when the milestone index last advanced.
static BOOT_VIEW_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
/// First-load latch: the initial game boot-to-title native loading-screen counters are sticky. Do not
/// let those stale "already reached 100%" semaphores drive the user-started load bar.
static BOOT_VIEW_FIRST_LOAD_REQUEST_REARMED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub(crate) use er_telemetry_core::counters::BOOT_VIEW_IDX_CHANGED_MS;

/// Optional, pre-decoded local screenshot background, resolved once per process.
///
/// Where to look, how to decode it, and the two pixel passes that put it behind the bar all live in
/// `er-boot-background`. That code reads no game memory, writes no counter and installs no hook, so
/// it had no claim on the DLL crate; what stays here is the cache and the host's answer to "where do
/// I look" -- the configured path and the game directory.
static BOOT_BG_IMAGE: std::sync::OnceLock<Option<er_boot_background::BootBgImage>> =
    std::sync::OnceLock::new();

/// The phase sequence THIS load epoch publishes. Boot walks engine bring-up through the first world;
/// a character reload cannot replay any of that, so it publishes a strictly smaller sequence and its
/// visible `N/M` denominator shrinks accordingly (bd er-effects-rs-ok8d).
fn boot_view_phase_set() -> &'static er_loading_bar_core::PhaseSet {
    if BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst) == BOOT_VIEW_EPOCH_KIND_RELOAD {
        &er_loading_bar_core::RELOAD_PHASE_SET
    } else {
        &er_loading_bar_core::BOOT_PHASE_SET
    }
}
/// Fill edge the bar pauses at while the startup save picker holds the boot (the MAIN MENU phase, whose
/// creep tops out near here). `boot_view_progress` clamps the fill here while a pick is pending, then
/// lifts it the frame the pick clears the latch.
const BOOT_VIEW_SAVE_CHECK_PERMILLE: usize = 470;
/// Asymptotic creep time-constant: creep = gap * since/(since + K). At `since == K` the bar is halfway to
/// the next milestone; it keeps approaching but never reaches it, so the bar NEVER fully freezes during a
/// long phase (user 2026-07-15: STARTING UP ~23s and the title load ~32s made a 70%-capped bar look stuck).
/// Seamless handoff (user 2026-07-06, replacing the earlier fade-out design): at the loading
/// handoff the cover HOLDS fully lit over the game's black gap and the loading screen's own
/// fade-in-from-black, then stops in a single cut once the native loading screen is fully lit --
/// a lit-to-lit scene cut with no black and no fade. Measured (run 194254 pixel telemetry): the
/// native fade-in luminance plateaus around CS::LoadingScreen update hit ~12, ~1.8s after the
/// loading-table build.
const BOOT_VIEW_NATIVE_LIT_UPDATE_HITS: usize = 12;
/// After the native loading close/result and a render-ready player, keep a black overlay and fade it
/// away over the live world. This covers the final native loading fade/black edge without popping from
/// a full loading-cover frame straight into gameplay.
const BOOT_VIEW_RELEASE_FADE_MS: u64 = 640;
/// The native now-loading GFx movie plays its own `FadeOut` label over ~15 frames at 30fps
/// (root black-plate alpha 239 -> 0, frames 105..119). Then the game may still have the loading
/// movie/background as the last presented backbuffer. Hold opaque until both the authored fade window
/// and the native loading update stream have been quiet long enough that our later alpha fade reveals
/// gameplay instead of the loading art.
const BOOT_VIEW_NATIVE_GFX_FADEOUT_HOLD_MS: u64 = 600;
const BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS: u64 = 900;
/// Largest single-frame step the fade's pause accumulator will credit.
///
/// The step is a frame period, and Elden Ring presents as slowly as ~5 fps while loading, so 250 ms
/// accepts every real frame. Its job is the pathological one: if the render thread is descheduled
/// for seconds between two paused frames, that whole gap must not be credited as pause and silently
/// stretch a 640 ms fade into a multi-second one.
const BOOT_VIEW_FADE_HOLD_MAX_STEP_MS: u64 = 250;
/// Total pause the release fade will absorb before it proceeds regardless.
///
/// A honored hold clears once `CS::LoadingScreen::Update` has been quiet for
/// `BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS`, so a genuine one is always well inside this. The cap
/// exists because the fade is the ONLY exit from a world-handoff cover window, so an unbounded
/// pause would be an unbounded cover and the per-frame GPU readback behind it would sit on the
/// frame rate forever.
///
/// CORRECTION (2026-08-30). The reasoning above is sound but it only ever asked the question one
/// way round: what bounds the window once `world_handoff` is TRUE. It never asked the converse --
/// what bounds the window when `world_handoff` is NEVER true -- and the answer was NOTHING. This
/// cap is not, and cannot become, a lifetime bound: a fade that is never ENTERED is never paused,
/// so there is nothing here to cap. That hole is what [`boot_view_absolute_backstop`] closes; the
/// two are complementary, this one bounding a running fade and that one bounding the window.
///
/// CORRECTION (2026-08-31). The paragraph above this one used to add "the FPS bail below is
/// reachable only through the `own_menu_active` branch this code never falls to once
/// `world_handoff` is true". That is FALSE, and run br-20260831-160354-2513 measured it false:
/// `world_handoff` went true at 224208 ms, the native-loading hold kept the fade from STARTING, and
/// every frame fell straight past the `native_gfx_hold_pending && !fade_started` arm into the FPS
/// bail, which stopped the window at 239858 ms with `reason=2`. A true `world_handoff` does not
/// keep this code out of the bail; only a fade that has actually STARTED does. That is why the
/// composite cap has to be able to tell a finished gauge from a frozen one
/// ([`er_loading_portrait_core::native_loading_progress`]) rather than relying on never being
/// reached.
const BOOT_VIEW_FADE_MAX_HELD_MS: u64 = 2_000;

// ---- ABSOLUTE COVER BACKSTOP (user report 2026-08-30) ----
//
// THRESHOLDS, CHOSEN FROM MEASURED NUMBERS RATHER THAN TASTE. A backstop that fires during a
// legitimately slow load is a REGRESSION, not a safety net: it would tear the cover off mid-load
// and show the user the black gap the cover exists to hide. Every value below is therefore set
// well past the slowest legitimate load this repo has evidence for.
//
//   * the reported failure's own load: `handoff_seen_ms = 19117`, last real phase label `+20449ms`;
//   * `BOOT_VIEW_EPOCH_COMPOSITE_CAP_MS` = 20_000, the own-menu epoch's existing composite cap;
//   * `RELOAD_STALL_BACKSTOP_SECONDS = 120.0` in `scripts/capture-samechar-3x.py`, the repo's
//     existing DEFENSIVE reload backstop, whose comment reads "Generous so a slow-but-progressing
//     load is never cut" -- the same job, the same reasoning, so the same number;
//   * `BOOT_TIMEOUT_SECONDS = 300.0` in the same file, raised from 110 precisely because ER's
//     asset load is slow-but-progressing under load and must not be torn down early.
/// Wall-clock lifetime of a single cover window, measured from `BOOT_VIEW_WINDOW_ARM_MS` (the
/// user's load REQUEST, not process start). The pure last resort: it is the only arm that still
/// works when the world clock itself cannot be read.
///
/// 120 s is ~6x the reported failure's 19.1 s handoff and 6x the own-menu composite cap, matches
/// the repo's existing 120 s reload-stall backstop, and sits well inside the 300 s boot budget --
/// so a slow-but-progressing load cannot reach it, while a window with no exit at all cannot
/// outlive it. The user in the reported session waited 7+ minutes; this bounds that at two.
const BOOT_VIEW_BACKSTOP_LIFETIME_MS: u64 = 120_000;
/// How long `GameDataMan::play_time` must rise CONTINUOUSLY, while the cover is still opaque,
/// before the world counts as demonstrably live. This is the PRIMARY arm: it does not wait for a
/// clock to run out, it observes that the game is already being played underneath us.
///
/// 5 s against the repo's own `PLAY_TIME_LIVE_THRESHOLD_MS` of 1_000 (`input_trace.rs`,
/// `write_game_module_oracles.rs`), i.e. 5x the margin the rest of the codebase calls "the world
/// is genuinely live". A healthy release lands within ~1 s of the world coming up (both of its
/// latches are set by then and the fade is 640 ms), so five seconds of live world under an opaque
/// full-screen cover cannot be a healthy window. The reported failure ran 186 s past this.
const BOOT_VIEW_BACKSTOP_WORLD_LIVE_MS: u64 = 5_000;
/// Largest single-sample rise in `play_time` this treats as one frame of world simulation.
///
/// A presented frame cannot advance the world clock by two seconds -- Elden Ring presents as
/// slowly as ~5 fps under the cover's readback, which is 200 ms, so this is 10x headroom. Its real
/// job is the discontinuity: a character switch replaces the clock wholesale with a DIFFERENT
/// character's playtime, which can jump by hours in either direction, and crediting that jump as
/// world simulation would fire the backstop in the middle of the switch's load.
const BOOT_VIEW_BACKSTOP_PT_MAX_STEP_MS: usize = 2_000;
/// A world clock that has not risen for this long ends the current live run.
///
/// This is the repo's own definition of a load boundary read back the other way round: a sustained
/// flat `play_time` window IS a load (`er_telemetry_core::read::epoch`), so a run of rises
/// interrupted by one was not continuous gameplay and must not be summed across the gap.
const BOOT_VIEW_BACKSTOP_FLAT_RESET_MS: u64 = 1_000;
/// `BOOT_VIEW_BACKSTOP_TRIGGER` values (0 = never tripped).
const BOOT_VIEW_BACKSTOP_TRIGGER_WORLD_LIVE: usize = 1;
const BOOT_VIEW_BACKSTOP_TRIGGER_LIFETIME: usize = 2;

// Per-window backstop state. Internal sampling state only, cleared by
// `boot_view_reset_cover_window`; the counters a reader is meant to look at
// (`BOOT_VIEW_BACKSTOP_RELEASES` / `_FIRST_MS` / `_TRIGGER`) deliberately survive a rearm.
/// Epoch-ms at which the backstop tripped for the CURRENT window (0 = not tripped). A latch: once
/// set, the window is committed to the backstop release and a later flat sample cannot undo it.
static BOOT_VIEW_BACKSTOP_TRIPPED_MS: AtomicUsize = AtomicUsize::new(0);
/// Previous `play_time` sample (game ms; 0 = none yet).
static BOOT_VIEW_BACKSTOP_PT_LAST: AtomicUsize = AtomicUsize::new(0);
/// Boot-view epoch ms of the first and most recent rise in the current continuous-rise run.
static BOOT_VIEW_BACKSTOP_PT_RISE_FIRST_MS: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_BACKSTOP_PT_RISE_LAST_MS: AtomicUsize = AtomicUsize::new(0);

// Strip geometry (pixels; text is the 5x7 font at 2x = 10x14). ER-idiomatic minimal presentation
// (user 2026-07-05: the panel/border/percent styling clashed with the game): a hairline bar on a
// dark track near the bottom of the screen -- the game's own now-loading bar language -- with a
// small dim label above it. Everything else in the copied strip rect is pure black, which is
// indistinguishable from the black boot frames underneath, so only the bar + label are visible.
// (The game's REAL loading-bar widget/asset cannot be reused here: its menu resources are not in
// game memory until ~+12.7s and the DLL must not unpack assets from disk itself.)
pub(super) const BOOT_VIEW_TEXT_BASE_SCALE: usize = 2;
const BOOT_VIEW_TEXT_REFERENCE_H: u32 = 1080;
const BOOT_VIEW_TEXT_MIN_SCALE: usize = 1;
const BOOT_VIEW_TEXT_MAX_SCALE: usize = 4;
pub(crate) const BOOT_VIEW_GLYPH_H: usize = er_loading_bar_core::GLYPH_H;
/// Advance per character (5px glyph + 1px gap, pre-scale).
#[allow(dead_code)] // Retained: Glyph metric pair with the live BOOT_VIEW_GLYPH_H; kept so the two are read together.
pub(crate) const BOOT_VIEW_GLYPH_ADV: usize = er_loading_bar_core::GLYPH_ADV;
/// Hairline bar, like the game's own loading bar.
const BOOT_VIEW_BAR_H: usize = 3;
/// Gap between the text row and the bar track.
const BOOT_VIEW_TEXT_BAR_GAP: usize = 5;
/// Bottom padding row so the handoff marker never touches the strip edge.
const BOOT_VIEW_PAD_BOTTOM: usize = 3;
/// Total strip height: text row, gap, bar, bottom pad.
fn boot_view_strip_height(text_scale: usize) -> usize {
    BOOT_VIEW_GLYPH_H * text_scale + BOOT_VIEW_TEXT_BAR_GAP + BOOT_VIEW_BAR_H + BOOT_VIEW_PAD_BOTTOM
}

fn boot_view_text_scale(backbuffer_h: u32) -> usize {
    let scaled = (backbuffer_h as usize * BOOT_VIEW_TEXT_BASE_SCALE
        + (BOOT_VIEW_TEXT_REFERENCE_H as usize / 2))
        / BOOT_VIEW_TEXT_REFERENCE_H as usize;
    scaled.clamp(BOOT_VIEW_TEXT_MIN_SCALE, BOOT_VIEW_TEXT_MAX_SCALE)
}
/// Strip width = backbuffer width * NUM/DEN (clamped to a sane minimum).
const BOOT_VIEW_STRIP_W_NUM: u32 = 19;
const BOOT_VIEW_STRIP_W_DEN: u32 = 25;
const BOOT_VIEW_STRIP_MIN_W: u32 = 220;
/// Strip top edge = backbuffer height * NUM/DEN (near the bottom, where the game's own bar lives).
const BOOT_VIEW_STRIP_Y_NUM: u32 = 91;
const BOOT_VIEW_STRIP_Y_DEN: u32 = 100;

// Palette (R, G, B) -- the game's understated loading-bar language: off-white hairline fill over a
// near-black track, dim warm-grey caption text. Black elsewhere (invisible over the boot frames).
const BOOT_VIEW_RGB_BLACK: [u8; 3] = [0, 0, 0];
const BOOT_VIEW_RGB_TRACK: [u8; 3] = [26, 26, 26];
const BOOT_VIEW_RGB_FILL: [u8; 3] = [226, 223, 214];
const BOOT_VIEW_RGB_TEXT: [u8; 3] = [150, 147, 138];

/// Delta of a process-STICKY counter since this epoch's baseline. A reload epoch must never assert a
/// phase from `!= 0` on a counter a previous load already moved.
fn boot_view_epoch_delta(
    counter: &'static std::sync::atomic::AtomicUsize,
    baseline: &'static std::sync::atomic::AtomicUsize,
) -> bool {
    counter.load(Ordering::SeqCst) != baseline.load(Ordering::SeqCst)
}

/// True once THIS epoch's load request has been committed. All four sources are sticky for the whole
/// process, so each is measured against its rearm-time baseline (bd er-effects-rs-ok8d: the unbaselined
/// `!= 0` form re-latched LOADING SAVE on the reload's very first frame).
fn boot_view_load_confirmed_this_epoch() -> bool {
    boot_view_epoch_delta(
        &SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT,
        &BOOT_VIEW_CONTINUE_ALLOW_BASELINE,
    ) || boot_view_epoch_delta(&TFC_CONTINUE_FIRED, &BOOT_VIEW_TFC_CONTINUE_BASELINE)
        || boot_view_epoch_delta(
            &LOADING_BG_PORTRAIT_SPARED_RENDERER,
            &BOOT_VIEW_PORTRAIT_SPARED_BASELINE,
        )
        || boot_view_epoch_delta(
            &SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT,
            &BOOT_VIEW_FRESH_DESER_BASELINE,
        )
}

/// True once `phase`'s semaphore has asserted FOR THE CURRENT EPOCH. Every predicate is a pure
/// atomic/pointer read that is safe from the render thread. Keyed on the phase IDENTITY rather than a
/// table index so the boot and reload sequences cannot drift apart: a phase means the same thing in
/// both, it just occupies a different slot.
fn boot_phase_reached(phase: er_loading_bar_core::LoadPhase) -> bool {
    use er_loading_bar_core::LoadPhase as P;
    let reload = BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst) == BOOT_VIEW_EPOCH_KIND_RELOAD;
    let quick_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    match phase {
        // Drawing at all proves the present hook + game swapchain are live.
        P::StartingUp => true,
        // A reload epoch exists BECAUSE a slot was confirmed; the rearm is that confirmation.
        P::SwitchConfirmed => true,
        P::GameSystems => game_man_ptr_or_null() != 0,
        // ACQUIRING ASSETS: the title menu starts acquiring its Scaleform resources (~12.7s), right
        // after GameMan. First of three title-asset ramps; splitting the old single ~32s "asset load"
        // label into three keeps the bar/label advancing across that long stretch.
        P::AcquiringAssets => TITLE_MENU_RESOURCE_ACQUIRE_HITS.load(Ordering::SeqCst) != 0,
        // OPENING / BUILDING MENU UI: the .gfx file-open counter climbs to ~113 across the load, so
        // keying these off ASCENDING COUNT thresholds (not `!= 0`) spreads the two labels through the
        // stretch instead of both flipping the instant the first file opens.
        P::OpeningMenuUi => TITLE_SCALEFORM_FILE_OPEN_HITS.load(Ordering::SeqCst) >= 30,
        P::BuildingMenuUi => TITLE_SCALEFORM_FILE_OPEN_HITS.load(Ordering::SeqCst) >= 70,
        // RETURNING TO TITLE: the switch wrote menuData+0x5d and the quickload FSM left the confirm.
        P::ReturningToTitle => quick_phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
        P::TitleReady => {
            if reload {
                // The switch FSM owns this on a reload. The boot-only PRESS START / fade-in-skip
                // latches below are sticky for the whole process and would assert instantly.
                quick_phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
            } else {
                // PRESS START is bound (~40s, the title is actually up internally) -- OR the fade-in
                // skip fired (backstop). We cover the title itself; this only reflects the engine
                // reaching it.
                TITLE_PRESS_START_BIND_HITS.load(Ordering::SeqCst) != 0
                    || TITLE_FADEIN_SKIP_FIRED.load(Ordering::SeqCst)
                        != TITLE_OWNER_SCAN_START_ADDRESS
            }
        }
        P::PreparingSave => {
            if reload {
                quick_phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
            } else {
                // Menu opened internally -- the own-stepper latch when that task runs, OR'd with the
                // network-check shortcircuit which fires ~10ms after the title-accept-byte natural
                // menu-open on the product path.
                PRODUCT_CORE_LAST_MENU_OPENED_LATCH.load(Ordering::SeqCst) != 0
                    || NETWORK_CHECK_SHORTCIRCUIT_COUNT.load(Ordering::SeqCst) != 0
            }
        }
        P::LoadingSave => boot_view_load_confirmed_this_epoch(),
        // World-load phases, keyed off the game's native CS::LoadingScreen so they assert on every
        // load path. The counters behind them are cleared by the epoch reset, so they are already
        // epoch-relative. The boot epoch additionally gates on a real load request so the bar cannot
        // sit at 100% from the boot-to-title loading screen before the user starts loading.
        P::BuildingWorld | P::StreamingWorld | P::FinalizingWorld | P::EnteringWorld => {
            (reload || boot_view_load_flow_requested()) && boot_world_phase_reached(phase)
        }
    }
}

/// The world-load phases, keyed off the game's native CS::LoadingScreen so they assert on every load
/// path (normal autoload AND own-menu switch), independent of the profile-table build. Pure atomic
/// reads; safe from the render thread.
fn boot_world_phase_reached(phase: er_loading_bar_core::LoadPhase) -> bool {
    use er_loading_bar_core::LoadPhase as P;
    let update_hits = LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst);
    let progress = LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst);
    let close_hits = LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst);
    match phase {
        // BUILDING WORLD: the native loading screen has appeared -> the world build has begun.
        P::BuildingWorld => update_hits != 0,
        // STREAMING WORLD: the native world-load gauge is actively streaming.
        P::StreamingWorld => progress > 0,
        // FINALIZING WORLD: the gauge is past the midpoint, splitting the long stream into two labels.
        P::FinalizingWorld => progress >= 500,
        // ENTERING WORLD: the gauge is near-complete, or a close arrived on a gauge that is finished
        // (or that never existed).
        //
        // A bare `close_hits != 0` is NOT enough. A reload's loading screen sends a close while its
        // gauge is still at frame 1/500 -- a transient screen closing, not the world handing off -- and
        // trusting it fired ENTERING WORLD ~1s into the reload, jumping the fill from 48% to 91% and
        // collapsing STREAMING/FINALIZING WORLD into the same instant (measured run
        // samechar-3x-threedll-20260730-082930, bd er-effects-rs-ok8d).
        P::EnteringWorld => {
            let max_frame = LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst);
            let cur_frame = LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst);
            // Through the shared definition, not a local copy. The copy at the release predicate
            // below was never made in the first place, and that omission is er-effects-rs-t7q2.
            progress >= 900 || (close_hits != 0 && np::gauge_done(cur_frame, max_frame))
        }
        _ => false,
    }
}

fn boot_view_player_render_ready() -> bool {
    let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
        return false;
    };
    let chr_model_ins = player.chr_ins.chr_model_ins.as_ptr() as usize;
    let chr_ctrl = player.chr_ins.chr_ctrl.as_ptr() as usize;
    chr_model_ins != TITLE_OWNER_SCAN_START_ADDRESS
        && chr_ctrl != TITLE_OWNER_SCAN_START_ADDRESS
        && player.chr_ins.chr_flags1c4.is_render_group_enabled()
        && player.chr_ins.chr_flags1c5.enable_render()
}

/// The local player exists and carries a real loaded character (model + controller both built).
///
/// Weaker than [`boot_view_player_render_ready`] on purpose: it says a character is RESIDENT, not
/// that it is currently render-enabled, and unlike the render flags it is PERSISTENT once true
/// rather than a ~2 s spike at the tail of a load. Same pointer walk, no hook anywhere in it.
fn boot_view_player_loaded() -> bool {
    unsafe { PlayerIns::local_player_mut() }
        .map(|player| {
            player.chr_ins.chr_model_ins.as_ptr() as usize != TITLE_OWNER_SCAN_START_ADDRESS
                && player.chr_ins.chr_ctrl.as_ptr() as usize != TITLE_OWNER_SCAN_START_ADDRESS
        })
        .unwrap_or(false)
}

/// Has the cover's job finished -- is the game ready to be revealed?
///
/// The two facts that answer this are "the player model is render-enabled" and "the native loading
/// screen is finishing". Both occur in a normal session, but at DIFFERENT times: `render_ready` is
/// an instantaneous read of the player's render flags and goes true early (measured +27491ms in run
/// slot-portrait-proof-20260731-115718), while the native close/fadeout lands seconds later. The
/// previous predicate `render_ready && (close_sent || permille >= 998)` required them in the SAME
/// frame, so it never fired in product -- all 57 `boot-view DECISION` lines of that run read
/// `world_handoff=false` while the loads completed normally. With no reachable release, the FPS bail
/// became the cover's only stop, and its heuristic predicate is what the user saw as the vanilla
/// loading screen flashing through (er-effects-rs-drb7).
///
/// So latch each fact for the window and release once BOTH have been observed. `can_move_handoff`
/// stays as an immediate release: it is the strongest possible proof the world is playable, but it
/// is written only by the PROOF-ONLY can-move probe (`can_move_probe.rs:277` -- "never fires in a
/// normal user session"), so it can never be the product path on its own.
/// Native loading-screen SHOWINGS a System->Quit->Load Character switch produces: the outgoing
/// world's teardown plate, then the incoming character's load plate. The boot load produces one, so
/// this threshold is only ever applied to a switch.
const BOOT_VIEW_SWITCH_NATIVE_SCREENS: usize = 2;

fn boot_view_cover_release_ready(can_move_handoff: bool) -> bool {
    use er_telemetry_core::counters::{
        BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN, BOOT_VIEW_RELEASE_READY_MS,
        BOOT_VIEW_RELEASE_RENDER_READY_SEEN, BOOT_VIEW_SEMANTIC_RELEASES,
    };
    // SECOND-SCREEN GATE, AND IT IS DELIBERATELY ABOVE `can_move_handoff` (2026-09-05).
    //
    // `can_move_handoff` short-circuiting first is what actually released the cover, and it is a
    // false positive by construction rather than a rare race. It reads `CAN_MOVE_CONFIRMED &&
    // MOVE_PROBE_EPOCH == cur_load_epoch`, and at the ARM frame of a switch BOTH still describe the
    // OUTGOING world: the player is genuinely still standing in it, and neither epoch counter has
    // moved yet. Measured at the arm frame of two separate switches on
    // br-20260905-234626-ce9a -- `can_move_handoff=true fresh_deser=0 move_epoch=0` -- so
    // `world_handoff` was true on the cover window's FIRST frame, the fade started immediately, and
    // `stop cover ... cover_window_ms=689` landed 350 ms before the incoming loading screen opened.
    // The character-load gate below was written for exactly this and never got to run.
    //
    // The gate here is the one the user described in product terms: cover from the moment the first
    // plate comes up to the moment the second fades out. `LOADING_SCREEN_CLOSE_SENT_HITS` is reset
    // per cover window and latched once per native showing, so `>= 2` IS "the character load's own
    // screen has finished". On the one switch whose cover survived long enough to log the whole
    // sequence that moment was +178101ms, 12.9 s after the arm -- the correct release, against the
    // 682 ms the code actually took.
    if er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_SECOND_SCREEN.load(Ordering::SeqCst)
        != 0
    {
        let screens = LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst);
        er_telemetry_core::counters::BOOT_VIEW_NATIVE_SCREENS_SEEN.store(screens, Ordering::SeqCst);
        if screens < BOOT_VIEW_SWITCH_NATIVE_SCREENS {
            er_telemetry_core::counters::BOOT_VIEW_RELEASE_HELD_FOR_SECOND_SCREEN
                .fetch_add(1, Ordering::SeqCst);
            // Same reason the confirm gate clears its latches: both release facts are readily true
            // during the teardown plate, so one observed then would fire the release the instant
            // this gate opened -- the identical bug, one screen later.
            BOOT_VIEW_RELEASE_RENDER_READY_SEEN.store(0, Ordering::SeqCst);
            BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.store(0, Ordering::SeqCst);
            return false;
        }
        // AND IT RELEASES. It does not fall through to the gates below, and that is the whole
        // correction of 2026-09-05's second attempt.
        //
        // WHAT FALLING THROUGH COST, measured on br-20260905-235624-a149. The span half worked --
        // the cover held across both plates, `LS FINISH hits=1 frame=1/500` at +86721ms (the unload)
        // and `hits=2 frame=500/500` at +103626ms (the load) -- and then the cover NEVER CAME DOWN.
        // Not late: never. It stayed up 54 s, until the next switch's rearm at +157331ms tore the
        // window down for its own reasons, and the user heard the character load in behind it.
        //
        // The blocker was `BOOT_VIEW_RELEASE_REQUIRE_CONFIRM` immediately below, which holds until
        // `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT` advances past its arm-time baseline. On
        // the USER-driven ProfileSelect path that counter never moves: `fresh_deser=0` in every one
        // of that run's DECISION lines, from the first to the last, so `0 <= 0` held forever while
        // the bar sat at `permille=1000` and `render_ready=true`.
        //
        // The two gates were asking the same question. The confirm counter is a PROXY for "this
        // switch's character load has begun"; the second plate FINISHING is that load having
        // finished, observed directly, on the same path the user is on. A proxy that disagrees with
        // the thing it stands for is not a second opinion, so the stronger fact answers for both.
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.store(0, Ordering::SeqCst);
        return true;
    }
    if can_move_handoff {
        return true;
    }
    // CHARACTER-LOAD GATE (er-effects-rs-q6vk). A switch shows TWO native loading screens -- the
    // return-to-title teardown, then the character load after continue_confirm -- and BOTH satisfy
    // the two facts below. Measured (run slot-portrait-proof-20260731-122038): the cover armed at
    // ~25490ms, released at 28047ms while still in the teardown, and the character load that began
    // at ~42513ms ran uncovered. So hold the release until THIS switch's character load has
    // actually begun: the fresh-deser count bumps at the reload's deserialize, and is documented to
    // NOT have incremented yet at arm time (see the composite-clock note in
    // `rearm_boot_progress_for_own_menu_load`).
    //
    // While held, the latches are CLEARED rather than left standing. Both facts are readily true
    // during the teardown/title -- the player is still resident and the teardown bar fills -- so a
    // latch kept from that phase would fire the release the instant the confirm landed, which is
    // the same bug one screen later. They must be observed again against the character load.
    if er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.load(Ordering::SeqCst) != 0 {
        let baseline =
            er_telemetry_core::counters::BOOT_VIEW_RELEASE_CONFIRM_BASELINE.load(Ordering::SeqCst);
        let fresh_deser =
            crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        if fresh_deser <= baseline {
            er_telemetry_core::counters::BOOT_VIEW_RELEASE_HELD_FOR_CONFIRM
                .fetch_add(1, Ordering::SeqCst);
            BOOT_VIEW_RELEASE_RENDER_READY_SEEN.store(0, Ordering::SeqCst);
            BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.store(0, Ordering::SeqCst);
            return false;
        }
    }
    // "The game is ready to be revealed." This needs a signal that is still TRUE once the
    // character-load gate above opens, which rules out the obvious one.
    //
    // `boot_view_player_render_ready()` is not wrong, it is TRANSIENT: measured across runs
    // 20260731-115718, -122038 and -125800 it reads true for only ~2 sampled seconds at the TAIL of
    // each load (permille 990-1000) and false everywhere else, twice per load. The gate is still
    // holding during that window -- it clears both latches every frame until this switch's
    // fresh-deser bump -- so the transient is discarded, and it never comes back. Gating on it
    // alone therefore left the release permanently unsatisfiable once the teardown path was closed:
    // 0 semantic releases, both switches riding the 20s cap (run -125800).
    //
    // (An earlier revision of this comment called it "inverted, true at the title and false
    // in-world". That was drawn from the last 8 DECISION lines of one run -- all post-load steady
    // state -- and is wrong. The full logs show the tail-of-load spikes above.)
    //
    // The per-epoch world-live signal is the semantically ideal answer -- play time advancing for
    // THIS load epoch is the game saying the world is running, and it is what the Present hook's
    // in-world composite skip already trusts -- but it is not reachable in every run: it needs play
    // time to ADVANCE past a threshold, and a probe that tears down shortly after the world appears
    // never gets there (runs -125800 and -130326 both ended `play_time_live=false`,
    // `play_time_advanced_ms=0`). Kept as the preferred signal for real sessions, not relied on.
    let cur_load_epoch =
        crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    let epoch_world_live = cur_load_epoch != 0
        && crate::constants::BOOT_VIEW_EPOCH_WORLD_LIVE.load(Ordering::SeqCst) == cur_load_epoch;
    // The fact that IS both reachable and PERSISTENT -- so it survives the gate's hold window,
    // unlike the render-flag spike -- is the one the proof harness itself trusts to declare LOADED:
    // the local player exists and carries a real loaded character. Runs -125800 and -130326 both
    // ended `player_present=true` with a valid name/level while world-live stayed false. Reading it
    // here in-process is the same evidence one layer earlier.
    //
    // It cannot reveal the game early despite being broad: the character-load gate above still
    // blocks everything before this switch's confirm, and the release also requires the native-done
    // latch, which only sets when the load screen is actually finishing.
    let player_loaded = boot_view_player_loaded();
    if epoch_world_live || player_loaded || boot_view_player_render_ready() {
        BOOT_VIEW_RELEASE_RENDER_READY_SEEN.store(1, Ordering::SeqCst);
    }
    // The game's own "this loading screen is going away" signals.
    //
    // The Scaleform GFx fadeout is deliberately NOT one of them. It looked like the earliest honest
    // end-of-screen signal, but the run that validated this release proves it contributed nothing:
    // in both windows the window's first fadeout landed AFTER the release had already fired
    // (release 28047ms vs fadeout 28569ms; release 52351ms vs fadeout 52875ms) -- the release
    // latched on the native bar instead. It is also unsafe here. `scaleform_label_goto_hook`
    // stamps on any timeline label merely CONTAINING "fadeout", and a burst of 64 such stamps
    // lands during the return-to-title transition (42834ms in that run). Once the cover is armed
    // earlier to close the re-arm gap (er-effects-rs-q6vk), that burst would latch native-done at
    // window OPEN, and with the player still resident from the previous world render-ready latches
    // too -- releasing the cover instantly and defeating that fix.
    //
    // AND IT MUST ASK THE GAUGE (er-effects-rs-t7q2). This read `close_hits != 0 || permille >= 998`
    // until 2026-08-31 -- the exact bare-close form `boot_world_phase_reached(EnteringWorld)` above
    // already refuses, and refuses for the same measured reason: a reload's transient loading screen
    // sends its finish while its gauge is still at frame 1 of 500. In run br-20260831-160354-2513
    // that close (boot-view 217892 ms, `frame=1/500`) latched this release at 224208 ms, 17 s before
    // the gauge reached 500/500 -- and because `boot_view_absolute_backstop` stands down while the
    // release is reachable, it also DISARMED the window's 120 s lifetime bound. Both sites now go
    // through `np::release_native_done` / `np::gauge_done`, so they cannot drift apart again.
    if np::release_native_done(
        LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst),
        LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst),
        LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst),
        LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst),
    ) {
        BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.store(1, Ordering::SeqCst);
    }
    let ready = BOOT_VIEW_RELEASE_RENDER_READY_SEEN.load(Ordering::SeqCst) != 0
        && BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.load(Ordering::SeqCst) != 0;
    if ready && BOOT_VIEW_RELEASE_READY_MS.load(Ordering::SeqCst) == 0 {
        let now_ms = boot_view_epoch_ms().max(1) as usize;
        BOOT_VIEW_RELEASE_READY_MS.store(now_ms, Ordering::SeqCst);
        let n = BOOT_VIEW_SEMANTIC_RELEASES.fetch_add(1, Ordering::SeqCst) + 1;
        // A4: a release that lands without its switch's character load having begun is the q6vk
        // defect recurring. Counted, not merely logged, so the harness can gate on it.
        let require =
            er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.load(Ordering::SeqCst);
        let baseline =
            er_telemetry_core::counters::BOOT_VIEW_RELEASE_CONFIRM_BASELINE.load(Ordering::SeqCst);
        let fresh_deser =
            crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        let before_confirm = require != 0 && fresh_deser <= baseline;
        if before_confirm {
            er_telemetry_core::counters::BOOT_VIEW_RELEASE_BEFORE_CONFIRM
                .fetch_add(1, Ordering::SeqCst);
        }
        append_autoload_debug(format_args!(
            "boot-view: COVER RELEASE #{n} at {now_ms}ms -- render-ready and native loading screen finishing both latched this window (the real handoff; no bail needed) require_confirm={require} fresh_deser={fresh_deser} baseline={baseline} before_confirm={before_confirm}"
        ));
    }
    ready
}

/// Wall-clock ms over which `GameDataMan::play_time` has been rising CONTINUOUSLY, sampled once per
/// composited frame. 0 whenever there is no such run.
///
/// A total delta would have been simpler and wrong. The baseline for a delta has to come from
/// somewhere, and on an own-menu switch the clock at arm time belongs to the OUTGOING character:
/// when the incoming one has more playtime the delta jumps by hours the instant it deserializes --
/// mid-load, with the loading screen still up. Measuring the SPAN OF A RUN OF PLAUSIBLE RISES
/// instead makes that jump self-cancelling: it is one implausible step, the run resets, and the
/// clock has to earn its five seconds again from the world that is actually running.
fn boot_view_backstop_world_live_dwell_ms(now_ms: u64) -> u64 {
    let gdm = crate::game_data_man_ptr_or_null();
    // Pointer walk off the game's own singleton -- the same class of read as
    // `boot_view_player_render_ready`, and deliberately NOT any counter a detour writes.
    let play_time = if gdm == 0 {
        0
    } else {
        unsafe {
            crate::experiments::safe_read_usize(gdm + crate::GAME_DATA_MAN_PLAY_TIME_A0_OFFSET)
        }
        .map_or(0, |value| value & 0xffff_ffff)
    };
    let prev = BOOT_VIEW_BACKSTOP_PT_LAST.swap(play_time, Ordering::SeqCst);
    let now = now_ms.max(1) as usize;
    let step = play_time.saturating_sub(prev);
    let rose = play_time != 0 && prev != 0 && step != 0;
    // Unreadable, gone backwards, or moved further in one frame than a frame can move it: this is
    // not a clock we can reason about (no character yet, or a different one). Drop the run.
    if play_time == 0 || play_time < prev || step > BOOT_VIEW_BACKSTOP_PT_MAX_STEP_MS {
        BOOT_VIEW_BACKSTOP_PT_RISE_FIRST_MS.store(0, Ordering::SeqCst);
        BOOT_VIEW_BACKSTOP_PT_RISE_LAST_MS.store(0, Ordering::SeqCst);
        return 0;
    }
    if rose {
        BOOT_VIEW_BACKSTOP_PT_RISE_LAST_MS.store(now, Ordering::SeqCst);
        let _ = BOOT_VIEW_BACKSTOP_PT_RISE_FIRST_MS.compare_exchange(
            0,
            now,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    let first = BOOT_VIEW_BACKSTOP_PT_RISE_FIRST_MS.load(Ordering::SeqCst) as u64;
    let last = BOOT_VIEW_BACKSTOP_PT_RISE_LAST_MS.load(Ordering::SeqCst) as u64;
    if first == 0 || last == 0 {
        return 0;
    }
    // A step of 0 is ordinary (two presents inside one game tick) and must not end the run on its
    // own; a SUSTAINED flat window is the load boundary, and it does.
    if now_ms.saturating_sub(last) >= BOOT_VIEW_BACKSTOP_FLAT_RESET_MS {
        BOOT_VIEW_BACKSTOP_PT_RISE_FIRST_MS.store(0, Ordering::SeqCst);
        BOOT_VIEW_BACKSTOP_PT_RISE_LAST_MS.store(0, Ordering::SeqCst);
        return 0;
    }
    last.saturating_sub(first)
}

/// ABSOLUTE BACKSTOP: has this cover window failed so completely that it must be torn down by
/// force? Returns the per-window latch, so once true it stays true until the window re-arms.
///
/// THE HOLE THIS FILLS (user report 2026-08-30). A session played 7+ minutes with this cover
/// black-clearing the whole backbuffer over live gameplay and no way out but killing the process.
/// The proximate cause was a game-image detour that failed to install -- but the reason a single
/// failed detour was CATASTROPHIC rather than cosmetic is structural: on the boot epoch this
/// window had no backstop of any kind, because both of its exits are downstream of that same
/// detour.
///
///   * The release fade needs `world_handoff`, i.e. [`boot_view_cover_release_ready`], whose
///     `NATIVE_DONE_SEEN` half is written ONLY from `LOADING_SCREEN_CLOSE_SENT_HITS` or
///     `LOADING_SCREEN_BAR_PROGRESS_PERMILLE`, both fed by the CS::LoadingScreen detour. Measured
///     in the reported run: `release_native_done_seen = 0`, `fade_hits = 0`, `fade_start_ms = 0`.
///   * The FPS bail lives inside `if own_menu_active`, and the boot epoch is not an own-menu one
///     (`oracle_boot_view_epoch_kind = 0`), so that whole block is skipped.
///   * `can_move_handoff` is not a third exit: it needs `CAN_MOVE_CONFIRMED`, which only the
///     proof-only can-move probe writes, and its own comment says it never fires in a normal
///     session.
///
/// So: `stop_ms = 0`, `stop_reason = 0`, `semantic_releases = 0`, and
/// `present_full_clear_hits = 5001` against `present_hook_hits = 4794` -- full-clearing essentially
/// every Present, indefinitely.
///
/// WHY THESE SIGNALS AND NOT THE OBVIOUS ONES. A backstop keyed on anything a detour writes would
/// share the failure mode it exists to survive, which is the whole defect repeated one layer up.
/// Everything below is a pointer walk into game memory:
///
///   * `GameDataMan::play_time` rising is the game itself saying the world is simulating. It is
///     flat during loads -- this repo detects load boundaries BY that flatness
///     (`er_telemetry_core::read::epoch`) -- so it cannot be true during the load the cover is for.
///   * the local player carrying a loaded character. The same class of read as the render-ready
///     path that DEMONSTRABLY kept working throughout the failure
///     (`release_render_ready_seen = 1` is exactly such a read succeeding).
///
/// The wall clock is the second arm and the pure last resort: it is the only one left when the
/// world clock itself cannot be read at all.
///
/// IT CANNOT PREEMPT A HEALTHY RELEASE. `release_reachable` is `world_handoff`; while that is true
/// the normal path owns the window and this only samples, never trips. And the thresholds are set
/// so far past any measured legitimate load (see `BOOT_VIEW_BACKSTOP_LIFETIME_MS`) that a
/// slow-but-progressing load reaches neither arm.
fn boot_view_absolute_backstop(now_ms: u64, release_reachable: bool) -> bool {
    // Sample every frame, including the frames the healthy path owns: the dwell is the span of a
    // CONTINUOUS run of rises, so skipping frames would put holes in the run it measures.
    let world_live_ms = boot_view_backstop_world_live_dwell_ms(now_ms);
    if BOOT_VIEW_BACKSTOP_TRIPPED_MS.load(Ordering::SeqCst) != 0 {
        return true;
    }
    if release_reachable {
        return false;
    }
    // SWITCH GATE, reusing the q6vk character-load gate rather than inventing a second one. An
    // own-menu switch arms this window from the IN-WORLD System menu, so the OUTGOING world is
    // still simulating when the cover goes up and its play_time keeps rising through the
    // return-to-title teardown. Crediting that as "the world is live underneath our cover" would
    // fire the world-live arm on the teardown and reveal the exact screen the cover exists to
    // hide. So on a window that is waiting for a character load, the world-live arm stays shut
    // until that load has actually begun -- the same fresh-deser advance
    // `boot_view_cover_release_ready` holds its release on. The boot epoch clears
    // `BOOT_VIEW_RELEASE_REQUIRE_CONFIRM`, so it is unaffected. The wall clock below is NOT gated
    // this way: it is the last resort and must survive a switch that never deserializes at all.
    let confirm_pending = er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM
        .load(Ordering::SeqCst)
        != 0
        && crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst)
            <= er_telemetry_core::counters::BOOT_VIEW_RELEASE_CONFIRM_BASELINE
                .load(Ordering::SeqCst);
    let world_live = !confirm_pending
        && world_live_ms >= BOOT_VIEW_BACKSTOP_WORLD_LIVE_MS
        && boot_view_player_loaded();
    // From the load REQUEST, not process start: `BOOT_VIEW_WINDOW_ARM_MS` is 0 until
    // `boot_view_reset_cover_window` stamps it, and on the boot epoch that is the frame the user's
    // load flow was requested. A 0 here means the cover is still over the pre-Continue title,
    // where dwelling as long as the user likes is correct behaviour and not a defect to cut short.
    let arm_ms = BOOT_VIEW_WINDOW_ARM_MS.load(Ordering::SeqCst) as u64;
    let armed_ms = if arm_ms == 0 {
        0
    } else {
        now_ms.saturating_sub(arm_ms)
    };
    let lifetime = armed_ms >= BOOT_VIEW_BACKSTOP_LIFETIME_MS;
    if !(world_live || lifetime) {
        return false;
    }
    let now = now_ms.max(1) as usize;
    if BOOT_VIEW_BACKSTOP_TRIPPED_MS
        .compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // Another Present thread tripped it on the same frame; it owns the log line.
        return true;
    }
    let trigger = if world_live {
        BOOT_VIEW_BACKSTOP_TRIGGER_WORLD_LIVE
    } else {
        BOOT_VIEW_BACKSTOP_TRIGGER_LIFETIME
    };
    er_telemetry_core::counters::BOOT_VIEW_BACKSTOP_TRIGGER.store(trigger, Ordering::SeqCst);
    let _ = er_telemetry_core::counters::BOOT_VIEW_BACKSTOP_FIRST_MS.compare_exchange(
        0,
        now,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    let n =
        er_telemetry_core::counters::BOOT_VIEW_BACKSTOP_RELEASES.fetch_add(1, Ordering::SeqCst) + 1;
    // Once per window by construction (the latch above), and a window that reaches here is already
    // a defect -- so this is the one place in the cover's per-frame path where a full log line is
    // unconditionally worth the IO.
    append_autoload_debug(format_args!(
        "boot-view: ABSOLUTE BACKSTOP #{n} at {now}ms -- THIS IS A DEFECT, NOT A HEALTHY RELEASE. The cover window had no reachable exit and is being force-released through the fade (trigger={trigger} world_live_ms={world_live_ms} armed_ms={armed_ms} arm_ms={arm_ms} epoch_kind={} epoch_seq={} own_menu={} render_ready_seen={} native_done_seen={} handoff_seen_ms={} fade_start_ms={} draws={} full_clears={} permille={}). native_done_seen=0 here means the CS::LoadingScreen detour never wrote; investigate the hook install, not the cover.",
        BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst),
        BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst),
        BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst),
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_RENDER_READY_SEEN.load(Ordering::SeqCst),
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.load(Ordering::SeqCst),
        BOOT_VIEW_HANDOFF_SEEN_MS.load(Ordering::SeqCst),
        BOOT_VIEW_FADE_START_MS.load(Ordering::SeqCst),
        BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
        BOOT_VIEW_PRESENT_FULL_CLEAR_HITS.load(Ordering::SeqCst),
        BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
    ));
    true
}

fn boot_view_load_flow_requested() -> bool {
    BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0
        || SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst) != 0
        || TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0
        || OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst)
        || OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.load(Ordering::SeqCst) != 0
        || TFC_FORCED_CONTINUE_HANDOFF_MS.load(Ordering::SeqCst) != 0
        || SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) != 0
        || LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) != 0
}

/// Clear the native CS::LoadingScreen counters so the NEXT loading window is measured on its own.
/// These are cumulative for the whole process: without this the world phases inherit the previous
/// screen's finished state and the bar stays full with a frozen label for the entire next load
/// (user-reported 2026-07-16: "I don't know what's going on for 30s").
fn boot_view_reset_native_loading_semaphores() {
    LOADING_SCREEN_UPDATE_HITS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_BAR_PROGRESS_PERMILLE.store(0, Ordering::SeqCst);
    LOADING_SCREEN_BAR_CURRENT_FRAME.store(0, Ordering::SeqCst);
    LOADING_SCREEN_BAR_MAX_FRAME.store(0, Ordering::SeqCst);
    LOADING_SCREEN_CLOSE_SENT_HITS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_CLOSE_SENT.store(0, Ordering::SeqCst);
    LOADING_SCREEN_UPDATE_LAST_MS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_CLOSE_SENT_FIRST_MS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_GFX_FADEOUT_HITS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_GFX_FADEOUT_FOREIGN_HITS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_GFX_FADEOUT_FIRST_MS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_GFX_FADEOUT_LAST_MS.store(0, Ordering::SeqCst);
}

/// ONE-WAY RELEASE FADE (user report 2026-08-22, second round: "I still see my portrait come back
/// very briefly if I press escape too quickly after getting in game").
///
/// THE MECHANISM, confirmed in source rather than assumed. `native_gfx_hold_pending` is rebuilt
/// every frame from two RECENCY predicates -- a Scaleform fade-out stamped within
/// `BOOT_VIEW_NATIVE_GFX_FADEOUT_HOLD_MS`, or a `CS::LoadingScreen::Update` tick within
/// `BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS`. Neither can become true again on its own as time
/// passes; both need a FRESH stamp. One of the two writers used to take any stamp at all:
/// `scaleform_label_goto_hook` called `stamp_loading_gfx_fadeout` for ANY timeline label merely
/// CONTAINING "fadeout", on ANY movie, matched case-insensitively (`bounded_ascii_contains`
/// lowercases). Opening the in-world menu was enough, and that is measured rather than assumed: of
/// the 106 vanilla menu `.gfx` movies in the local extraction, 98 carry a frame label literally
/// named `FadeOut` -- including `02_000_ingametop.gfx`, the pause menu Escape opens, whose label
/// table reads `FadeIn` / `Loop` / `FadeOut`, and `01_000_fe.gfx`, the HUD that fades out when it
/// does. So the stamp was not a loading-screen signal at all; almost any menu transition refreshed
/// it. The hold then re-asserted mid-fade and the old code fell through to the OPAQUE cover path,
/// which rasterizes with
/// `draw_portrait: true` where the fade frame is built with `draw_portrait: false` -- so the head
/// was not riding the fade down, it was being re-drawn at full alpha, and then the fade finished
/// and took it away again. Reappear and tear down, exactly as reported.
///
/// THE RULE. The hold was written as a START GATE ("is it safe to begin fading yet?") and it is now
/// only asked as one. Once `BOOT_VIEW_FADE_START_MS` is set, the fade owns the rest of the window
/// and can never hand it back to the opaque path. But the gate is not simply switched off, because
/// it defends a real case: fading out while the game's own loading screen is still on the
/// backbuffer is the vanilla flash-through (er-effects-rs-wmw defect #1). So the two halves are
/// separated by what they can actually prove:
///
///   * the Scaleform half is a start gate ONLY. NARROWED 2026-09-05: it now stamps only for
///     `LOADING_SCREEN_LAST_THIS + LOADING_SCREEN_FADEOUT_CLIP_OFFSET`, the one clip the image
///     shows the screen playing its own fade on, so it CAN tell the loading screen's fade from a
///     menu's. It stays a start gate anyway -- the fade is one-way because of the fall-through
///     bug below, not because the signal was noisy -- and every other movie's label now lands in
///     `LOADING_SCREEN_GFX_FADEOUT_FOREIGN_HITS`, which holds nothing.
///   * the `CS::LoadingScreen::Update` half stays live during the fade, but only for ticks past
///     `BOOT_VIEW_FADE_START_LS_UPDATE_HITS`. Only the loading-screen detour writes that counter, so
///     a tick past the snapshot IS the game's loading screen running again, with no ambiguity.
///
/// A honored hold PAUSES the fade at its current alpha (see [`boot_view_fade_hold_tick`]) instead of
/// cancelling it, so the cover stays as opaque as it already was, never brighter, and the full ramp
/// still plays once the loading screen goes quiet.
///
/// WHY A GENUINE HOLD IS ALMOST UNREACHABLE HERE, and why that is not an argument for skipping it.
/// The fade only starts once the loading screen has been quiet for 900 ms, and a genuinely new load
/// re-arms the cover window -- `boot_view_reset_cover_window` clears `BOOT_VIEW_FADE_START_MS`, so
/// "mid-fade" cannot survive into a new load. The honored path is therefore expected to stay at 0,
/// which is precisely why it is CHEAP to keep: it costs one comparison per fade frame and removes
/// the need to argue that no such case exists.
fn boot_view_note_fade_hold_reassert(
    now_ms: usize,
    honored: bool,
    fadeout_pending: bool,
    update_quiet_pending: bool,
    ls_ticked_since_fade_start: bool,
) {
    let n = BOOT_VIEW_FADE_HOLD_REASSERTS.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = BOOT_VIEW_FADE_HOLD_REASSERTS_FIRST_MS.compare_exchange(
        0,
        now_ms,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    if honored {
        BOOT_VIEW_FADE_HOLD_HONORED.fetch_add(1, Ordering::SeqCst);
    } else {
        BOOT_VIEW_FADE_HOLD_REFUSED.fetch_add(1, Ordering::SeqCst);
    }
    // First frame of each contiguous run only. A single stamp keeps the recency window true for up
    // to 600 ms, which at 60 fps is ~36 frames from ONE Escape press; this is inside Present, so
    // that would be a log line per frame of a render stall. One press, one line.
    if BOOT_VIEW_FADE_HOLD_REASSERT_RUN.fetch_add(1, Ordering::SeqCst) != 0 {
        return;
    }
    append_autoload_debug(format_args!(
        "boot-view: FADE HOLD RE-ASSERT #{n} at {now_ms}ms -- the native-loading hold went true again {}ms into a release fade that had already started (honored={honored} fadeout_pending={fadeout_pending} update_quiet_pending={update_quiet_pending} ls_ticked_since_fade_start={ls_ticked_since_fade_start} alpha={} held_ms={}); refused holds are an over-matched Scaleform \"fadeout\" label from some other movie and the fade carries on, honored ones pause it",
        (now_ms as u64).saturating_sub(BOOT_VIEW_FADE_START_MS.load(Ordering::SeqCst) as u64),
        BOOT_VIEW_FADE_LAST_ALPHA.load(Ordering::SeqCst),
        BOOT_VIEW_FADE_HELD_MS.load(Ordering::SeqCst),
    ));
}

/// Accumulate the time the release fade spends PAUSED by an honored hold, and return the total the
/// fade clock should subtract.
///
/// Differencing consecutive paused frames, rather than timing the hold from its start, is what makes
/// this safe to call from Present: there is no state to unwind if the hold ends on a frame that
/// never runs, and a hold that stops and restarts simply contributes two runs. `_TICK_MS` is cleared
/// on every unpaused frame so the first frame of a new pause contributes nothing (it has no
/// predecessor to difference against) instead of crediting the whole gap since the last pause.
///
/// The returned total is clamped to `BOOT_VIEW_FADE_MAX_HELD_MS` while the stored one is not: the
/// cap is a safety bound on the fade, not a claim about how long the game held us, and the honest
/// number is the one worth reading afterwards.
fn boot_view_fade_hold_tick(now_ms: u64, held: bool) -> u64 {
    let stamp = if held { now_ms.max(1) as usize } else { 0 };
    let prev = BOOT_VIEW_FADE_HOLD_TICK_MS.swap(stamp, Ordering::SeqCst) as u64;
    if !held || prev == 0 {
        return (BOOT_VIEW_FADE_HELD_MS.load(Ordering::SeqCst) as u64)
            .min(BOOT_VIEW_FADE_MAX_HELD_MS);
    }
    let step = now_ms
        .saturating_sub(prev)
        .min(BOOT_VIEW_FADE_HOLD_MAX_STEP_MS);
    let total = BOOT_VIEW_FADE_HELD_MS.fetch_add(step as usize, Ordering::SeqCst) as u64 + step;
    total.min(BOOT_VIEW_FADE_MAX_HELD_MS)
}

/// Count an OPAQUE cover draw that reached the backbuffer while this window's release fade was
/// already running and had not yet completed.
///
/// THIS IS THE NUMBER THE PREVIOUS ROUND OF DETECTORS COULD NOT PRODUCE. Both of them --
/// [`boot_view_note_draw_after_stop`] and the `cover_plate_visible_after_release` watch -- open only
/// once `BOOT_VIEW_STOPPED` latches, and the reported defect happens BEFORE that, during the fade.
/// So they were structurally incapable of firing on it and duly returned 0, which read as "our
/// compositor did not draw what the user saw". It did. In run br-20260822-184123-fa3d the menu
/// opened at log +39905 ms, inside a fade window of +39281 to +40554, and across it
/// `boot_view_draw_hits` went 528 -> 538: ten opaque draws this counter would have named
/// immediately.
///
/// Expected 0 now that the fade is one-way; a nonzero value means a path back to the opaque
/// rasterizer survived, and `_FIRST_MS` minus `oracle_boot_view_fade_start_ms` says how far into the
/// fade it was.
fn boot_view_note_nonfade_draw_during_fade() {
    if BOOT_VIEW_FADE_START_MS.load(Ordering::SeqCst) == 0
        || BOOT_VIEW_STOPPED.load(Ordering::SeqCst) != 0
    {
        return;
    }
    let n = BOOT_VIEW_NONFADE_DRAW_DURING_FADE.fetch_add(1, Ordering::SeqCst) + 1;
    let now_ms = boot_view_epoch_ms().max(1) as usize;
    let _ = BOOT_VIEW_NONFADE_DRAW_DURING_FADE_FIRST_MS.compare_exchange(
        0,
        now_ms,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    // First of the window only: this runs inside Present, and if the fade has lost the frame once it
    // will lose every frame until it completes.
    if n == 1 {
        append_autoload_debug(format_args!(
            "boot-view: NON-FADE DRAW DURING FADE at {now_ms}ms -- the OPAQUE cover path (draw_portrait=true) drew {}ms into the release fade, which is the 2026-08-22 \"portrait comes back briefly\" defect (fade_start_ms={} last_alpha={} hold_reasserts={} refused={} honored={})",
            (now_ms as u64).saturating_sub(BOOT_VIEW_FADE_START_MS.load(Ordering::SeqCst) as u64),
            BOOT_VIEW_FADE_START_MS.load(Ordering::SeqCst),
            BOOT_VIEW_FADE_LAST_ALPHA.load(Ordering::SeqCst),
            BOOT_VIEW_FADE_HOLD_REASSERTS.load(Ordering::SeqCst),
            BOOT_VIEW_FADE_HOLD_REFUSED.load(Ordering::SeqCst),
            BOOT_VIEW_FADE_HOLD_HONORED.load(Ordering::SeqCst),
        ));
    }
}

/// NULL DETECTOR for the 2026-08-22 "loading screen briefly reappears after Escape" report.
///
/// Called at BOTH boot-view composite counter sites, reading `BOOT_VIEW_STOPPED` BEFORE the frame
/// makes its own store, so a nonzero read means the cover latched stopped on an EARLIER frame and
/// this one drew anyway. `BOOT_VIEW_STOPPED` deliberately, not `BOOT_VIEW_FADE_COMPLETE_MS`: the
/// FPS-bail exit never sets the latter, so a bail-stopped window would look armed forever.
///
/// It is expected to stay 0 for the life of the process, and that is precisely the point. The
/// diagnosis of that report rests on the claim that OUR compositor did not draw what the user saw
/// (`oracle_boot_view_stop_reason=1`, epoch_seq 0, fps_bail_resumes 0 -- none of the clearing paths
/// ran). If this ever fires, that claim is false and the next reader learns it from one number
/// instead of re-deriving it.
///
/// ONE KNOWN WAY IT COULD OVER-REPORT, so a single hit is read and not merely believed: both stop
/// stores happen BEFORE `BOOT_VIEW_DRAW_BUSY` is taken, so if the self-present pump thread and the
/// render thread were ever inside the composite together, one could latch the stop while the other
/// was already past the entry guard. That needs the pump still running at a release fade, and the
/// pump stops at the game's first present ~30 s earlier -- but the log line carries `stop_ms` for
/// exactly this reason: a same-instant stop is that race, a stop seconds earlier is not.
fn boot_view_note_draw_after_stop(site: &str) {
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) == 0 {
        return;
    }
    let n =
        er_telemetry_core::counters::BOOT_VIEW_DRAW_AFTER_STOP.fetch_add(1, Ordering::SeqCst) + 1;
    er_telemetry_core::counters::BOOT_VIEW_DRAW_AFTER_STOP_TOTAL.fetch_add(1, Ordering::SeqCst);
    let now_ms = boot_view_epoch_ms().max(1) as usize;
    let _ = er_telemetry_core::counters::BOOT_VIEW_DRAW_AFTER_STOP_FIRST_MS.compare_exchange(
        0,
        now_ms,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    // First of the window only. This runs inside Present; if the latch is ever wrong it is wrong
    // every frame, and a per-frame log would turn a render stall into an IO storm.
    if n == 1 {
        append_autoload_debug(format_args!(
            "boot-view: DRAW AFTER STOP at {now_ms}ms site={site} -- the cover composited a frame with BOOT_VIEW_STOPPED already set (stop_reason={} stop_ms={} epoch_seq={}); the 2026-08-22 \"our compositor did not draw it\" diagnosis does not hold",
            BOOT_VIEW_STOP_REASON.load(Ordering::SeqCst),
            er_telemetry_core::counters::BOOT_VIEW_STOP_MS.load(Ordering::SeqCst),
            BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst),
        ));
    }
}

/// Snapshot everything the post-release watch measures deltas against, at the instant the cover
/// latches stopped. Called from BOTH stop sites (release fade and FPS bail) so the watch behaves
/// the same whichever way the window ended.
fn boot_view_stamp_stop_baselines(now_ms: usize) {
    er_telemetry_core::counters::BOOT_VIEW_STOP_MS.store(now_ms.max(1), Ordering::SeqCst);
    // A new watch starts with a clean RUN. The watch only samples inside its window, so a run left
    // part-way through when the previous window's watch expired would otherwise be continued by the
    // next one -- and `_max_run` is exactly the number that tells a brief reappearance apart from a
    // plate that never went down, so carrying a stale count into it corrupts the one reading it is
    // for. The cumulative frame/first/last counters are session totals and deliberately survive.
    er_telemetry_core::counters::COVER_PLATE_VISIBLE_AFTER_RELEASE_CUR_RUN
        .store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_STOP_LS_UPDATE_BASELINE.store(
        LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    er_telemetry_core::counters::BOOT_VIEW_STOP_LS_FADEOUT_BASELINE.store(
        LOADING_SCREEN_GFX_FADEOUT_HITS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    // The total-load witness, so a world load STARTED after this stop is a delta. Read only by
    // `er_telemetry_core::counters::cover_owns_current_loading_screen`, which carries the argument.
    er_telemetry_core::counters::BOOT_VIEW_STOP_LOAD_WITNESS.store(
        SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
}

/// Re-arm the cover DRAWING WINDOW: stop latch, window clock, handoff/fade/dark-gap state, draw cache.
/// This is "start covering again", independent of whether the phase walk is also starting over.
fn boot_view_reset_cover_window() {
    BOOT_VIEW_STOPPED.store(0, Ordering::SeqCst);
    BOOT_VIEW_STOP_REASON.store(0, Ordering::SeqCst);
    // Post-stop draw detector + the post-release watch's baselines belong to ONE window.
    er_telemetry_core::counters::BOOT_VIEW_DRAW_AFTER_STOP.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_DRAW_AFTER_STOP_FIRST_MS.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_STOP_MS.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_STOP_LS_UPDATE_BASELINE.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_STOP_LS_FADEOUT_BASELINE.store(0, Ordering::SeqCst);
    BOOT_VIEW_WINDOW_ARM_MS.store(boot_view_epoch_ms().max(1) as usize, Ordering::SeqCst);
    BOOT_VIEW_FPS_BAIL_RESUMED.store(0, Ordering::SeqCst);
    BOOT_VIEW_HANDOFF_SEEN_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE.store(0, Ordering::SeqCst);
    BOOT_VIEW_STOP_NATIVE_HITS.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_START_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_COMPLETE_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_HITS.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_LAST_ALPHA.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_FAILURES.store(0, Ordering::SeqCst);
    // One-way-fade state. All per-window: the snapshot describes THIS window's fade start, and the
    // pause accumulator must not carry a previous window's pause into the next fade's clock.
    // The re-assert TALLIES (`BOOT_VIEW_FADE_HOLD_REASSERTS` / `_REFUSED` / `_HONORED` / `_FIRST_MS`)
    // and `BOOT_VIEW_NONFADE_DRAW_DURING_FADE*` are deliberately NOT cleared here, for the reason
    // given at `BOOT_VIEW_DRAW_AFTER_STOP_TOTAL`: a detector a rearm can silently empty is not a
    // detector, and a run with several loads must still be able to report the first occurrence.
    BOOT_VIEW_FADE_START_LS_UPDATE_HITS.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_HELD_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_HOLD_TICK_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_FADE_HOLD_REASSERT_RUN.store(0, Ordering::SeqCst);
    BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS.store(0, Ordering::SeqCst);
    BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_DARK_GAP_FAILURES.store(0, Ordering::SeqCst);
    BOOT_VIEW_DARK_GAP_LAST_HELD_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS.store(0, Ordering::SeqCst);
    // Absolute-backstop SAMPLING state. The trip latch belongs to one window, and the world-clock
    // run must not span a rearm: the load the new window covers is exactly the gap in that run.
    // The `BOOT_VIEW_BACKSTOP_RELEASES` / `_FIRST_MS` / `_TRIGGER` counters are deliberately NOT
    // cleared here, for the reason given at `BOOT_VIEW_DRAW_AFTER_STOP_TOTAL`.
    BOOT_VIEW_BACKSTOP_TRIPPED_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_BACKSTOP_PT_LAST.store(0, Ordering::SeqCst);
    BOOT_VIEW_BACKSTOP_PT_RISE_FIRST_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_BACKSTOP_PT_RISE_LAST_MS.store(0, Ordering::SeqCst);
    // Release latches belong to the window that observed them.
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_RENDER_READY_SEEN.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_READY_MS.store(0, Ordering::SeqCst);
    // Default OFF: boot's single load has no teardown screen in front of it, so it must not wait
    // for a fresh-deser bump that will never come. `rearm_boot_progress_for_own_menu_load` turns it
    // back on after calling through here, which is the only path that faces two screens.
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_SECOND_SCREEN.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_NATIVE_SCREENS_SEEN.store(0, Ordering::SeqCst);
    // Draw cache: force a re-rasterize on the first frame of the new window.
    BOOT_VIEW_DRAWN_PERMILLE.store(usize::MAX, Ordering::SeqCst);
    BOOT_VIEW_DRAWN_IDX.store(usize::MAX, Ordering::SeqCst);
    BOOT_VIEW_DRAWN_BG_ACTIVE.store(usize::MAX, Ordering::SeqCst);
}

/// Start a NEW LOAD EPOCH: everything `boot_view_reset_cover_window` does, plus the state that only a
/// genuinely new load may reset -- the phase walk, the displayed fill, the label high-water, and the
/// baselines for counters that are sticky for the whole process.
///
/// THE EPOCH BOUNDARY IS THE REARM, NOT THE TEARDOWN (bd er-effects-rs-ok8d). A teardown-side clear
/// only runs on the path that actually tore down, and the cover has several exits (release fade, FPS
/// bail, world handoff, and a load abandoned mid-flight), so clearing there leaves half-cleared state
/// behind whenever an epoch ends by a path that was not carrying the clear. The rearm is the single
/// mandatory gate every drawing epoch passes through, so resetting here makes an epoch start from a
/// known-zero state no matter how -- or whether -- the previous one finished.
fn boot_view_reset_epoch_state(kind: usize) {
    let seq = BOOT_VIEW_EPOCH_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    BOOT_VIEW_EPOCH_KIND.store(kind, Ordering::SeqCst);
    boot_view_reset_cover_window();
    boot_view_reset_native_loading_semaphores();
    // Phase walk + displayed fill. The mask starts at literally zero: phase 0 latches on this epoch's
    // first sample and is REPORTED, instead of being pre-seeded and silently skipped.
    BOOT_VIEW_REACHED_MASK.store(0, Ordering::SeqCst);
    BOOT_VIEW_MILESTONE_IDX.store(0, Ordering::SeqCst);
    BOOT_VIEW_LAST_PERMILLE.store(0, Ordering::SeqCst);
    BOOT_VIEW_IDX_CHANGED_MS.store(boot_view_epoch_ms(), Ordering::SeqCst);
    // Label monotonic high-water, stamped with the epoch SEQ so a new epoch drops it wholesale. It used
    // to be keyed on the fresh-deser counter, which only bumps at the reload's DESERIALIZE -- seconds
    // into the load -- so the previous epoch's high-water was still clamping the new one and the visible
    // label bounced between two different denominators mid-load.
    BOOT_VIEW_MONO_EPOCH.store(seq, Ordering::SeqCst);
    BOOT_VIEW_MONO_ORD.store(0, Ordering::SeqCst);
    BOOT_VIEW_MONO_LABEL_PTR.store(0, Ordering::SeqCst);
    BOOT_VIEW_MONO_LABEL_LEN.store(0, Ordering::SeqCst);
    BOOT_VIEW_LAST_LABEL_HASH.store(0, Ordering::SeqCst);
    // BASELINES for counters that stay set for the whole process, so this epoch's phases assert from
    // what happens NEXT rather than from what a previous load left behind.
    BOOT_VIEW_CONTINUE_ALLOW_BASELINE.store(
        SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    BOOT_VIEW_TFC_CONTINUE_BASELINE
        .store(TFC_CONTINUE_FIRED.load(Ordering::SeqCst), Ordering::SeqCst);
    BOOT_VIEW_PORTRAIT_SPARED_BASELINE.store(
        LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    BOOT_VIEW_FRESH_DESER_BASELINE.store(
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
}

/// The boot epoch's user-started load begins mid-sequence, NOT at a new epoch: the same walk that
/// started at STARTING UP continues into LOADING SAVE and the world phases. So this re-arms the cover
/// WINDOW and drops the stale boot-to-title native loading counters, and deliberately leaves the phase
/// walk alone -- resetting it here would re-report every phase the boot already passed.
fn boot_view_rearm_for_first_load_request_if_needed() {
    if BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0
        || !boot_view_load_flow_requested()
        || BOOT_VIEW_FIRST_LOAD_REQUEST_REARMED
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }
    boot_view_reset_cover_window();
    boot_view_reset_native_loading_semaphores();
    append_autoload_debug(format_args!(
        "boot-view: first load request rearmed; cleared boot-to-title native loading semaphores (epoch {} kind=boot phases={})",
        BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst),
        boot_view_phase_set().len(),
    ));
}

/// Compute the current (milestone idx, displayed permille). Latches newly reached milestones into the
/// monotonic mask, stamps idx-change time for the creep, and never lets the displayed value decrease.
pub(crate) fn boot_view_epoch_ms() -> u64 {
    let epoch = *BOOT_VIEW_EPOCH.get_or_init(std::time::Instant::now);
    epoch.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// The same clock as [`boot_view_epoch_ms`], read WITHOUT starting it: `None` until boot-view code
/// has anchored the epoch.
///
/// The distinction is not pedantry. `boot_view_epoch_ms` anchors on first call, so a caller outside
/// the boot view that happens to run first would silently move the origin of the clock every
/// telemetry `*_ms` field is measured against -- rewriting the meaning of the whole run's timeline
/// to stamp one event. Callers that only want to READ the timeline (the in-game menu open stamp,
/// the clock map) use this and simply decline to stamp while the clock does not exist yet.
pub(crate) fn boot_view_epoch_ms_if_anchored() -> Option<u64> {
    BOOT_VIEW_EPOCH
        .get()
        .map(|epoch| epoch.elapsed().as_millis().min(u64::MAX as u128) as u64)
}

/// Reopen the first-start custom loading bar for an own-menu character switch. The original boot view
/// deliberately stops forever once the first loading window/world takes over; the custom System->Quit
/// ProfileSelect path reuses the title/autoload pipeline later in the same process, so it needs a
/// per-switch rearm with baselines for persistent portrait semaphores.
pub(crate) fn rearm_boot_progress_for_own_menu_load(selected_slot: i32, source: &str) {
    let slot_key = selected_slot.saturating_add(1).max(0) as usize;
    let table_baseline = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst);
    BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.store(slot_key, Ordering::SeqCst);
    BOOT_VIEW_LOADSCREEN_TABLE_BASELINE.store(table_baseline, Ordering::SeqCst);
    boot_view_reset_epoch_state(BOOT_VIEW_EPOCH_KIND_RELOAD);
    // FPS-bail composite clock: force re-init at the first composite of THIS window. The clock is
    // keyed on the fresh-deser epoch, which has NOT incremented yet at rearm time (it bumps at the
    // reload's deserialize), so switch #2+ inherited the PREVIOUS window's first-composite timestamp
    // and instantly tripped the 20s cap (measured run samechar-3x-threedll-20260729-203842: bail at
    // cover_window_ms=36 with composite_ms=24582). The usize::MAX sentinel never equals a real epoch,
    // so the next composite's swap re-stamps BOOT_VIEW_COMPOSITE_FIRST_MS.
    crate::constants::BOOT_VIEW_COMPOSITE_EPOCH.store(usize::MAX, Ordering::SeqCst);
    BOOT_VIEW_PUMP_STOP_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_PUMP_STOP_REASON.store(0, Ordering::SeqCst);
    // CHARACTER-LOAD RELEASE GATE (er-effects-rs-q6vk). This arm happens at the switch TRIGGER,
    // before the return-to-title teardown -- so the character load this cover exists for has not
    // started yet. Snapshot the fresh-deser count here; the release stays held until it advances,
    // which is exactly when the reload deserializes. Set AFTER boot_view_reset_epoch_state above,
    // which resets the per-window release latches.
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_CONFIRM_BASELINE.store(
        crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.store(1, Ordering::SeqCst);
    // SECOND-SCREEN GATE, armed here and only here: the BOOT load shows one native plate, a switch
    // shows two, so this is the one flow that needs it. `boot_view_reset_epoch_state` ran above and
    // zeroed `LOADING_SCREEN_CLOSE_SENT_HITS`, so the count this gate reads describes THIS switch.
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_SECOND_SCREEN.store(1, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_RELEASE_HELD_FOR_SECOND_SCREEN
        .store(0, Ordering::SeqCst);
    er_telemetry_core::counters::BOOT_VIEW_NATIVE_SCREENS_SEEN.store(0, Ordering::SeqCst);
    // Clear the PREVIOUS character's portrait/render state IMMEDIATELY when a new load arms (2026-07-16,
    // user-reported: the old character lingered on the new load screen). The portrait window is otherwise
    // only reset on load COMPLETION, so the just-loaded character carried into the NEXT switch's cover.
    // Resetting here rebinds the portrait pipeline for the incoming slot so the cover shows the new
    // character (or a clean black/bar) instead of the prior one. SAME-IDENTITY variant (bd
    // er-effects-rs-dpf6 Phase 3): when the incoming slot+name-hash matches the published head's
    // identity tag, the bridge + crop envelope are KEPT (a same-character reload cannot show a wrong
    // head); an identity mismatch keeps the full 2026-07-16/2026-07-06 clear above. Game thread
    // (confirm-press hook), so the summary-record identity read is safe here.
    unsafe { loading_portrait_window_reset_for_switch(selected_slot, "own-menu-switch-rearm") };
    append_autoload_debug(format_args!(
        "boot-view: rearmed for own-menu character load selected_slot={selected_slot} source={source} table_baseline={table_baseline} epoch={} kind=reload phases={}",
        BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst),
        boot_view_phase_set().len(),
    ));
}

fn boot_view_progress() -> (usize, usize) {
    boot_view_rearm_for_first_load_request_if_needed();
    let set = boot_view_phase_set();
    // Highest phase this epoch has evidence for, then DOWNWARD CLOSURE over 0..=highest.
    //
    // The sequence is strictly ordered, so reaching phase k proves the load passed through every phase
    // before it -- whether or not that phase's own detector ever published. Closing the run is what
    // makes the reported walk hole-free: the old "set bit i iff predicate i" form left gaps whenever a
    // later detector fired first, and the bar reported 7 -> 8 -> 11, never naming STREAMING WORLD or
    // FINALIZING WORLD even though the load plainly went through them (bd er-effects-rs-ok8d).
    // EPOCH FENCE, opened before the phase predicates are read and checked again before the result is
    // published. `boot_view_reset_epoch_state` runs on the GAME thread at the switch arm and zeroes
    // both the reached-mask and every `LOADING_SCREEN_*` counter the world-load predicates key on;
    // this samples on the RENDER thread. Without the fence a sample that read the OUTGOING load's
    // finished counters could store its mask AFTER the reset had zeroed it, and because the mask is
    // monotonic within an epoch that pins the whole switch at a bar it never earned.
    //
    // MEASURED, run br-20260905-221201-969c, three loads and three different amounts of phantom fill:
    //   switch #1 (+55.6s)  every one of the nine milestones fired in the SAME millisecond as the arm,
    //                       first line already `mask 0x1ff` -- the bar sat full at ENTERING WORLD 8/8
    //                       for the whole 4s the load actually took
    //   switch #2 (+71.0s)  seven fired at once, `mask 0x7f`, entering at STREAMING WORLD 6/8
    //   switch #3 (+387.0s) `mask 0x3`, a real walk: RETURNING TO TITLE -> TITLE READY -> ...
    // Three different answers for the same code path is the signature of a race, not of a phase
    // predicate that is merely too loose; the amount of inherited fill is just where the render thread
    // happened to be standing relative to the game thread's reset.
    let epoch_at_sample = BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst);
    let mut highest = 0usize;
    for i in 0..set.len() {
        if boot_phase_reached(set.phase(i)) {
            highest = i;
        }
    }
    // The epoch reset runs on the GAME thread while this samples on the RENDER thread, so a single
    // frame can straddle the swap and read a wider epoch's mask against a narrower epoch's phase set.
    // Masking to the active set's width keeps `idx` inside the set instead of resolving a phase this
    // epoch does not have; the next frame is consistent either way.
    let set_mask = (1usize << set.len()) - 1;
    let prev_mask = BOOT_VIEW_REACHED_MASK.load(Ordering::SeqCst) & set_mask;
    let mask = prev_mask | ((1usize << (highest + 1)) - 1);
    let idx = ((usize::BITS - 1 - mask.max(1).leading_zeros()) as usize).min(set.main_total());
    let now_ms = boot_view_epoch_ms();
    if mask != prev_mask && BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst) == epoch_at_sample {
        BOOT_VIEW_REACHED_MASK.store(mask, Ordering::SeqCst);
        // Report EVERY phase that just became reached, in order -- not only the new top one.
        for i in 0..set.len() {
            if mask & (1 << i) != 0 && prev_mask & (1 << i) == 0 {
                append_autoload_debug(format_args!(
                    "boot-view: milestone -> {} (idx {i}/{}, mask 0x{mask:x})",
                    set.label(i),
                    set.main_total(),
                ));
            }
        }
    }
    if BOOT_VIEW_MILESTONE_IDX.swap(idx, Ordering::SeqCst) != idx {
        BOOT_VIEW_IDX_CHANGED_MS.store(now_ms, Ordering::SeqCst);
    }
    let base = set.base_permille(idx);
    let next = set.next_permille(idx);
    let gap = next.saturating_sub(base);
    // SUBSTEP-DRIVEN FILL: the active phase's own parenthesized sub-progression paces the bar across
    // its span. This is the real pacing source -- the phase's substeps are concrete RAM semaphores, so
    // the fill tracks actual work instead of a clock. `sub_i` is 1-based (the substep IN PROGRESS), so
    // `sub_i - 1` is the count COMPLETED and the fill can never reach the next phase's target early.
    let (_, sub_i, sub_max) = boot_view_phase_submilestone(set.phase(idx));
    let done = sub_i.saturating_sub(1).min(sub_max);
    let sub_fill = base + gap * done / sub_max.max(1);
    // NEVER ESTIMATE (user directive 2026-09-04). An asymptotic clock-driven creep used to sit here
    // -- `gap * since / (since + BOOT_VIEW_CREEP_K_MS)` -- so that "a phase with no finer RAM
    // granularity still inches forward instead of freezing". It advanced the bar on WALL TIME, with
    // no relation to work performed, which makes it an estimate presented as a measurement. Its own
    // stated purpose was to stop a stalled phase LOOKING stalled: that is the one thing a progress
    // readout must never do, because this project's whole runtime debugging turns on telling a slow
    // load apart from a frozen one, and a bar that inches forward regardless erases the difference
    // for the user AND for the agent reading the log afterwards.
    //
    // What remains is `sub_fill`, which is measured: the active phase's substeps are RAM semaphores,
    // so the fill moves when the game does and holds when it does not. A phase with no substep
    // granularity now HOLDS at its base, and AGENTS.md already says how to render that honestly --
    // a labelled `<phase-specific label> 1/1` in the parenthesised sub-progression, codifying the
    // ignorance rather than smoothing over it.
    let pm = sub_fill.min(next).min(1000);
    // While the startup save picker holds the boot, clamp the fill so it PAUSES at the PREPARING SAVE edge
    // (the phase creep would otherwise drift past it); the clamp lifts the frame the pick clears the latch,
    // so the bar resumes toward LOADING SAVE / the world phases.
    let pm = if missing_save_selection_pending() {
        pm.min(BOOT_VIEW_SAVE_CHECK_PERMILLE)
    } else {
        pm
    };
    // WORLD-LOAD tail: once the world phases are the active region, drive the product bar from the
    // game's real Gauge_3 progress on ALL runtimes. The previous native-Windows-only gate made
    // Wine/user-launch keep showing the stale pre-handoff milestone (~46%) while the real loading gauge
    // reached 100%. The floor is THIS epoch's BUILDING WORLD target, so the gauge maps onto whichever
    // slice of the bar the world tail owns in this epoch's phase sequence.
    //
    // The trigger is the PHASE WALK reaching BUILDING WORLD (whose own predicate is the native loading
    // screen appearing), NOT the cover's separate handoff latch. That latch can assert within a frame
    // of a reload arming -- long before any world work starts -- and using it here yanked the fill up
    // to the world floor before the bar had reported a single pre-world phase.
    let native = LOADING_SCREEN_BAR_PROGRESS_PERMILLE
        .load(Ordering::SeqCst)
        .min(1000);
    let world_idx = set
        .index_of(er_loading_bar_core::LoadPhase::BuildingWorld)
        .unwrap_or(0);
    let pm = if idx >= world_idx || native > 0 {
        let floor = set.base_permille_of(er_loading_bar_core::LoadPhase::BuildingWorld);
        pm.max(floor + native * (1000 - floor) / 1000)
    } else {
        pm
    };
    // Monotonic display: an idx re-latch or timer wobble must never walk the bar backwards.
    let shown = BOOT_VIEW_LAST_PERMILLE
        .fetch_max(pm, Ordering::SeqCst)
        .max(pm);
    (idx, shown)
}

fn boot_view_label_hash(text: &str) -> usize {
    fnv1a64(text.as_bytes()) as usize
}

fn boot_view_single_submilestone(label: &'static str) -> (&'static str, usize, usize) {
    (label, 1, 1)
}

fn boot_view_counter_submilestone(
    label: &'static str,
    current: usize,
    max: usize,
    fallback: &'static str,
) -> (&'static str, usize, usize) {
    if current == 0 || max == 0 {
        boot_view_single_submilestone(fallback)
    } else {
        (label, current.min(max), max)
    }
}

fn boot_view_first_pending_substep(
    substeps: &[(bool, &'static str)],
) -> (&'static str, usize, usize) {
    let total = substeps.len().max(1);
    for (idx, (ok, label)) in substeps.iter().enumerate() {
        if !*ok {
            return (*label, idx + 1, total);
        }
    }
    ("COMPLETE", total, total)
}

fn boot_view_world_gauge_submilestone(fallback: &'static str) -> (&'static str, usize, usize) {
    let current = LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst);
    let max = LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst);
    if LOADING_SCREEN_BAR_ENABLED.load(Ordering::SeqCst) != 0 && max != 0 {
        ("WORLD LOADING", current.min(max), max)
    } else {
        boot_view_single_submilestone(fallback)
    }
}

fn boot_view_entering_world_submilestone() -> (&'static str, usize, usize) {
    let request_code = SWITCH_ORACLE_REQUEST_CODE.load(Ordering::SeqCst);
    let mms_step = SWITCH_ORACLE_MMS_STEP.load(Ordering::SeqCst);
    let current_epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    let bar_terminal = LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst) >= 998
        || (LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst) != 0
            && LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst)
                >= LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst));
    let ls730_held = SWITCH_ORACLE_LOADING_FIELD10.load(Ordering::SeqCst) != 0;
    let ls731_clear = SWITCH_ORACLE_LOADING_FIELD11.load(Ordering::SeqCst) == 0;
    let loading_close_sent = LOADING_SCREEN_CLOSE_SENT.load(Ordering::SeqCst) != 0
        || SWITCH_ORACLE_LOADING_FIELD11.load(Ordering::SeqCst) != 0;
    // The two post-FINISH InGameStep stages ride here, as substeps of the phase they belong to.
    let request_started = request_code >= INGAMESTEP_REQUEST_CODE_MAP_FINISH;
    let request_stable = request_code >= INGAMESTEP_REQUEST_CODE_IN_WORLD;
    let menu_job_present = SWITCH_ORACLE_MENU_JOB_PRESENT.load(Ordering::SeqCst) != 0;
    let player_present = SWITCH_ORACLE_PLAYER_PRESENT.load(Ordering::SeqCst) != 0;
    let movemap_done = request_stable || mms_step >= MOVEMAPSTEP_STEP_FINISH_INDEX;
    let movement_proven = CAN_MOVE_CONFIRMED.load(Ordering::SeqCst)
        && MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == current_epoch;
    if BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst) == BOOT_VIEW_EPOCH_KIND_RELOAD {
        boot_view_first_pending_substep(&[
            (bar_terminal, "LOAD BAR FULL"),
            (ls730_held, "LOAD SCREEN UP"),
            (ls731_clear, "CLOSE HANDSHAKE"),
            (movemap_done, "MAP STEP DONE"),
            (request_stable, "WORLD HANDOFF"),
            (menu_job_present, "GAME UI LIVE"),
            (player_present, "PLAYER IN WORLD"),
            (movement_proven, "CAN MOVE"),
        ])
    } else {
        boot_view_first_pending_substep(&[
            (bar_terminal, "LOAD BAR FULL"),
            (loading_close_sent, "CLOSING SCREEN"),
            (request_started, "MAP LOAD SENT"),
            (movemap_done, "MAP STEP DONE"),
            (request_stable, "WORLD HANDOFF"),
            (menu_job_present, "GAME UI LIVE"),
            (player_present, "PLAYER IN WORLD"),
            (movement_proven, "CAN MOVE"),
        ])
    }
}

fn boot_view_load_save_submilestone() -> (&'static str, usize, usize) {
    if BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst) == BOOT_VIEW_EPOCH_KIND_RELOAD {
        // All three counters are sticky for the process, so a reload reads them as deltas since this
        // epoch's rearm -- otherwise every substep reports COMPLETE on the reload's first frame.
        boot_view_first_pending_substep(&[
            (
                SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.load(Ordering::SeqCst) != 0,
                "SAVE SELECTED",
            ),
            (
                boot_view_epoch_delta(
                    &SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT,
                    &BOOT_VIEW_FRESH_DESER_BASELINE,
                ),
                "READING SAVE",
            ),
            (
                boot_view_epoch_delta(
                    &SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT,
                    &BOOT_VIEW_CONTINUE_ALLOW_BASELINE,
                ),
                "LOAD CONFIRMED",
            ),
        ])
    } else {
        let table_seen = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst)
            > BOOT_VIEW_LOADSCREEN_TABLE_BASELINE.load(Ordering::SeqCst);
        boot_view_first_pending_substep(&[
            (
                SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst) != 0
                    || TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0
                    || LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) != 0,
                "LOAD CONFIRMED",
            ),
            (table_seen, "SCREEN DATA READY"),
        ])
    }
}

/// STARTING UP (phase 0) sub-progression: the render/display bring-up chain, every step a concrete
/// RAM semaphore set by OUR OWN present path (swapchain resolve + Present detour install, our D3D12
/// command objects, our loading cover reaching the backbuffer, the game's own render loop presenting)
/// -- never the game's boot state, which is not yet observable this early. Each predicate ORs in every
/// LATER signal so an earlier step can never read false once a later stage has fired (keeps the
/// reported step monotonic even when a counter path is skipped, e.g. no self-present pump on Wine).
/// The reported step is the first not-yet-satisfied one (the stage in progress); once all are done we
/// hold the final milestone label. All labels are 5x7 font-safe (uppercase A-Z + space).
fn boot_view_starting_up_submilestone() -> (&'static str, usize, usize) {
    use er_telemetry_core::counters as c;
    // 1: the game swapchain is resolved and our Present detour is installed (the display path exists).
    let swapchain = c::GAME_SWAPCHAIN.load(Ordering::SeqCst) != 0
        || c::PRESENT_HOOK_INSTALLED.load(Ordering::SeqCst) != 0
        || BOOT_VIEW_SWAPCHAIN_FOUND_MS.load(Ordering::SeqCst) != 0
        || c::PRESENT_HOOK_HITS.load(Ordering::SeqCst) != 0;
    // 2: our own D3D12 device + command objects are up (derived from the backbuffer) -- we can draw.
    let device = BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst) == 1
        || BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst) != 0
        || BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst) != 0
        || c::PRESENT_HOOK_HITS.load(Ordering::SeqCst) != 0;
    // 3: our loading cover has actually reached the backbuffer / been presented at least once.
    let cover = BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst) != 0
        || BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst) != 0
        || c::PRESENT_HOOK_HITS.load(Ordering::SeqCst) != 0;
    // 4: the game's OWN render loop is presenting frames now (the engine is live behind our cover).
    let engine_frames = c::PRESENT_HOOK_HITS.load(Ordering::SeqCst) != 0
        || BOOT_VIEW_PUMP_STOP_REASON.load(Ordering::SeqCst) == 1;
    let steps: [(bool, &'static str); 4] = [
        (swapchain, "SWAPCHAIN"),
        (device, "RENDER DEVICE"),
        (cover, "COVER LIVE"),
        (engine_frames, "ENGINE FRAMES"),
    ];
    let mut current = steps.len();
    for (i, (ok, _)) in steps.iter().enumerate() {
        if !*ok {
            current = i + 1;
            break;
        }
    }
    (steps[current - 1].1, current, steps.len())
}

/// GAME SYSTEMS (phase 1) sub-progression = the game's OWN top-level boot state machine,
/// CS::CSSystemStep. Its `current_state` (states 0..20 of CSSystemStepState) names the exact
/// subsystem the boot thread is constructing / waiting on -- this IS the singleton + manager
/// construction sequence this phase covers, so the sublabel tracks the real substep instead of a
/// single placeholder. One guaranteed-ordered RAM read: base + global -> current_state (u32 low
/// dword), at the offset owned by `er_game_base::rva`.
/// The global staying null very early, or an out-of-range value, falls back to the coarse label so a
/// state number is never fabricated. RVA/state names from the Ghidra 1.16.2 dump (CSSystemStep
/// ctor @0x140dec7c0, singleton @base+0x3d85680) and the game's own 21-entry StepperFn table
/// (initializer 0x1400b16f0); the offset is MEASURED, see
/// `er_game_base::rva::CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` -- it was 0x40 here until 2026-08-31,
/// which read a pointer field and pinned this sub-progression to its fallback label.
/// The Wait* states map to the ctor child steps
/// (res/file/pad/sound/graphics). InitBoot/WaitBoot pairs are the early core-boot sub-phases (each
/// InitBootN kicks the work, the paired WaitBootN blocks on it) -- named generically because their
/// per-index subsystem was not statically pinned. Labels are 5x7 font-safe (uppercase + digits).
const BOOT_SYS_STEP_GLOBAL_RVA: usize = er_game_base::rva::CS_SYSTEM_STEP_GLOBAL_RVA;
const BOOT_SYS_STEP_STATE_OFFSET: usize = er_game_base::rva::CS_SYSTEM_STEP_CURRENT_STATE_OFFSET;
const BOOT_SYS_STEP_STATE_COUNT: usize = 21;
const BOOT_SYS_STEP_SUBLABELS: [&str; BOOT_SYS_STEP_STATE_COUNT] = [
    "SYSTEM INIT",     // 0  Init
    "CORE BOOT 1",     // 1  InitBoot1
    "CORE BOOT 1",     // 2  WaitBoot1
    "CORE BOOT 2",     // 3  InitBoot2
    "CORE BOOT 2",     // 4  WaitBoot2
    "CORE BOOT 3",     // 5  InitBoot3
    "CORE BOOT 3",     // 6  WaitBoot3
    "CORE BOOT 4",     // 7  InitBoot4
    "CORE BOOT 4",     // 8  WaitBoot4
    "CORE BOOT 5",     // 9  InitBoot5
    "CORE BOOT 5",     // 10 WaitBoot5
    "GAME FLOW INIT",  // 11 InitGameFlow
    "GAME FLOW WAIT",  // 12 WaitGameFlow
    "GAME FLOW DONE",  // 13 FinishGameFlow
    "PRE GRAPHICS",    // 14 WaitPreGraphics
    "GRAPHICS UP",     // 15 WaitGraphics
    "INPUT DEVICES",   // 16 WaitPad
    "RESOURCE SYSTEM", // 17 WaitRes
    "SOUND SYSTEM",    // 18 WaitSound
    "FILE SYSTEM",     // 19 WaitFile
    "SYSTEMS READY",   // 20 Finish
];

fn boot_view_game_systems_submilestone() -> (&'static str, usize, usize) {
    let total = BOOT_SYS_STEP_STATE_COUNT - 1; // 20 (Finish)
    let state = crate::experiments::game_module_base()
        .ok()
        .and_then(|base| unsafe {
            crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                BOOT_SYS_STEP_GLOBAL_RVA,
                "BOOT_SYS_STEP_GLOBAL_RVA",
            ))
        })
        .filter(|instance| *instance >= 0x10000)
        .and_then(|instance| unsafe {
            crate::experiments::safe_read_usize(instance + BOOT_SYS_STEP_STATE_OFFSET)
        })
        .map(|v| v & 0xffff_ffff);
    match state {
        Some(s) if s < BOOT_SYS_STEP_STATE_COUNT => (BOOT_SYS_STEP_SUBLABELS[s], s, total),
        // Not resolvable yet (very early) or unexpected: keep the coarse label, fabricate nothing.
        _ => boot_view_single_submilestone("GAME CORE UP"),
    }
}

/// SWITCHING SAVE (reload phase 0) sub-progression. Between the confirm press and the return-title
/// request there is no finer RAM granularity we can honestly claim, so this phase declares that
/// ignorance with a single explicit substep rather than borrowing a label from a neighbouring phase.
fn boot_view_switch_confirmed_submilestone() -> (&'static str, usize, usize) {
    boot_view_single_submilestone("SLOT CONFIRMED")
}

/// RETURNING TO TITLE (reload) sub-progression: the two switch-teardown semaphores that actually gate
/// this phase -- the menuData+0x5d teardown request we write, then the quickload FSM observing the
/// title owner. Both are phase-relevant: nothing about the incoming save can be true yet.
fn boot_view_returning_to_title_submilestone() -> (&'static str, usize, usize) {
    let quick_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    boot_view_first_pending_substep(&[
        (
            ENDING_REQUEST_SET.load(Ordering::SeqCst) != 0,
            "TEARDOWN REQUEST",
        ),
        (
            quick_phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN,
            "WORLD RELEASED",
        ),
    ])
}

/// Sub-progression for the ACTIVE phase, keyed on the phase identity so both epochs' sequences resolve
/// through the same table. Every returned label is a substep OF THAT PHASE: a phase with known RAM
/// granularity exposes its real substeps, and a phase without any exposes exactly one explicit
/// phase-specific substep (`<label> 1/1`) instead of borrowing an unrelated one.
fn boot_view_phase_submilestone(
    phase: er_loading_bar_core::LoadPhase,
) -> (&'static str, usize, usize) {
    use er_loading_bar_core::LoadPhase as P;
    match phase {
        P::StartingUp => boot_view_starting_up_submilestone(),
        P::GameSystems => boot_view_game_systems_submilestone(),
        P::AcquiringAssets => boot_view_counter_submilestone(
            "MENU FILES",
            TITLE_MENU_RESOURCE_ACQUIRE_HITS.load(Ordering::SeqCst),
            38,
            "MENU FILES",
        ),
        P::OpeningMenuUi => boot_view_counter_submilestone(
            "UI FILES",
            TITLE_SCALEFORM_FILE_OPEN_HITS.load(Ordering::SeqCst),
            113,
            "UI FILES",
        ),
        P::BuildingMenuUi => boot_view_counter_submilestone(
            "UI BUILD",
            TITLE_SCALEFORM_RESOURCE_CTOR_HITS.load(Ordering::SeqCst),
            112,
            "UI BUILD",
        ),
        P::SwitchConfirmed => boot_view_switch_confirmed_submilestone(),
        P::ReturningToTitle => boot_view_returning_to_title_submilestone(),
        P::TitleReady => {
            if BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst) == BOOT_VIEW_EPOCH_KIND_RELOAD {
                // The boot latches below are sticky; on a reload the switch FSM is the only honest
                // evidence that the title has come back up for THIS epoch.
                boot_view_single_submilestone("TITLE OWNER UP")
            } else {
                boot_view_first_pending_substep(&[
                    (
                        TITLE_PRESS_START_BIND_HITS.load(Ordering::SeqCst) != 0,
                        "PRESS START",
                    ),
                    (
                        TITLE_FADEIN_SKIP_FIRED.load(Ordering::SeqCst)
                            != TITLE_OWNER_SCAN_START_ADDRESS,
                        "TITLE UP",
                    ),
                ])
            }
        }
        P::PreparingSave => {
            if BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst) == BOOT_VIEW_EPOCH_KIND_RELOAD {
                boot_view_single_submilestone("AUTOLOAD HANDOFF")
            } else {
                boot_view_first_pending_substep(&[
                    (
                        PRODUCT_CORE_LAST_MENU_OPENED_LATCH.load(Ordering::SeqCst) != 0,
                        "MENU OPEN",
                    ),
                    (
                        NETWORK_CHECK_SHORTCIRCUIT_COUNT.load(Ordering::SeqCst) != 0,
                        "NETWORK CHECK",
                    ),
                ])
            }
        }
        P::LoadingSave => boot_view_load_save_submilestone(),
        P::BuildingWorld => boot_view_world_gauge_submilestone("LOAD SCREEN"),
        P::StreamingWorld | P::FinalizingWorld => {
            boot_view_world_gauge_submilestone("WORLD LOADING")
        }
        P::EnteringWorld => boot_view_entering_world_submilestone(),
    }
}

/// 5x7 glyphs for the milestone labels + percent readout. Each row byte uses bit 4 as the LEFTMOST
/// pixel. Hand-authored for this module (our own asset; nothing game-derived). Unknown chars render
/// as blanks rather than failing.
#[allow(dead_code)] // Retained: Measurement half of the boot-view text API, beside the live draw half.
pub(crate) fn boot_text_width(text: &str, scale: usize) -> usize {
    er_loading_bar_core::text_width(text, scale)
}

/// Blit `text` into the tight RGBA buffer at (x, y), scaled by `scale`.
// Argument-for-argument pass-through of `er_loading_bar_core::draw_text_rgb`. Grouping these into a
// struct would only unpack it again one line later; the argument count belongs to that API.
#[allow(clippy::too_many_arguments)]
pub(crate) fn boot_draw_text_rgb(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    text: &str,
    rgb: [u8; 3],
    scale: usize,
) {
    er_loading_bar_core::draw_text_rgb(buf, w, h, x, y, text, rgb, scale);
}

fn boot_draw_text(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    text: &str,
    scale: usize,
) {
    boot_draw_text_rgb(buf, w, h, x, y, text, BOOT_VIEW_RGB_TEXT, scale);
}

fn boot_draw_text_shadowed(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    text: &str,
    scale: usize,
) {
    boot_draw_text_rgb(
        buf,
        w,
        h,
        x.saturating_add(scale),
        y.saturating_add(scale),
        text,
        BOOT_VIEW_RGB_BLACK,
        scale,
    );
    boot_draw_text(buf, w, h, x, y, text, scale);
}

/// Axis-aligned opaque fill into the tight RGBA buffer (clamped).
// Argument-for-argument pass-through of `er_loading_bar_core::fill_rect_rgb`; same reasoning as
// `boot_draw_text_rgb` above.
#[allow(clippy::too_many_arguments)]
pub(super) fn boot_fill_rect(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    rw: usize,
    rh: usize,
    rgb: [u8; 3],
) {
    er_loading_bar_core::fill_rect_rgb(buf, w, h, x0, y0, rw, rh, rgb);
}

fn boot_bg_image() -> Option<&'static er_boot_background::BootBgImage> {
    BOOT_BG_IMAGE
        .get_or_init(|| {
            er_boot_background::load(
                &er_boot_background::Sources {
                    configured_image: crate::config::configured_boot_background_image(),
                    game_directory: game_directory_path(),
                },
                append_autoload_debug,
            )
        })
        .as_ref()
}

// BootViewFrame moved to er_loading_portrait_core::host (portrait crate split); the struct
// flows back in through the glob shim at the top of gpu_readback.rs, so this shared
// rasterizer still constructs the same type the crate's native overlay consumes.

/// Render the boot/loading-screen frame ONCE, device-agnostically: the loading bar (milestone label,
/// ticks, text scaling, progress creep) and -- when the startup save picker is armed -- its browser panel
/// composited on top. This is the SHARED rasterizer for BOTH the Wine in-swapchain composite
/// (composite_boot_progress_inner) and the native-Windows separate-window overlay, so the loading screen
/// is identical on both. The caller uploads `rgba` and copies it to its backbuffer at `(dx, dy)`.
pub(crate) fn boot_view_render_frame(bw: usize, bh: usize) -> BootViewFrame {
    let bw32 = bw as u32;
    let bh32 = bh as u32;
    let text_scale = boot_view_text_scale(bh32);
    let strip_w = (bw32 * BOOT_VIEW_STRIP_W_NUM / BOOT_VIEW_STRIP_W_DEN)
        .max(BOOT_VIEW_STRIP_MIN_W)
        .min(bw32);
    let strip_h = (boot_view_strip_height(text_scale) as u32).min(bh32);
    let strip_dx = (bw32 - strip_w) / 2;
    let strip_dy = (bh32 * BOOT_VIEW_STRIP_Y_NUM / BOOT_VIEW_STRIP_Y_DEN).min(bh32 - strip_h);
    let bg = boot_bg_image();
    let picker_active = save_picker_overlay_active();
    // Loading-screen character stats (game menu font) also need the full-screen canvas so they land at
    // their expected 5%/60% location; force full_frame when they are shown, exactly like picker_active.
    let stats_active = stats_overlay_active();
    // Captured character portrait (from LOADING_BG_PORTRAIT_RGBA) also needs the full-screen canvas so the
    // head lands at its upper-left rect; force full_frame when a portrait is published, like picker/stats.
    let portrait_active = portrait_overlay_active();
    let full_frame = bg.is_some() || picker_active || stats_active || portrait_active;
    let (region_w, region_h, dx, dy, content_x, content_y, content_w) = if full_frame {
        (
            bw,
            bh,
            0usize,
            0usize,
            strip_dx as usize,
            strip_dy as usize,
            strip_w as usize,
        )
    } else {
        (
            strip_w as usize,
            strip_h as usize,
            strip_dx as usize,
            strip_dy as usize,
            0usize,
            0usize,
            strip_w as usize,
        )
    };
    let (ms_idx, permille) = boot_view_progress();
    // Portrait draws INSIDE the rasterizer (behind the bar) when active and the picker is not up.
    let draw_portrait = portrait_active && !picker_active;
    let mut rgba = boot_view_rasterize(
        BootViewRaster {
            w: region_w,
            h: region_h,
            idx: ms_idx,
            permille,
            content_x,
            content_y,
            content_w,
            text_scale,
            draw_portrait,
        },
        bg,
    );
    if picker_active {
        // Picker owns the screen exclusively (no character context to portrait/stat yet).
        let _ = overlay_save_picker_onto(&mut rgba, region_w, region_h);
    } else if stats_active {
        // Stats stay in front of the portrait; the game-font block sits at 5%/60%.
        let _ = overlay_stats_onto(&mut rgba, region_w, region_h);
    }
    BootViewFrame {
        rgba,
        w: region_w,
        h: region_h,
        dx,
        dy,
    }
}

pub(crate) fn boot_view_d3d12_compositor_frame(
    bw: usize,
    bh: usize,
    _present_frame_index: usize,
) -> er_d3d12_compositor::CompositorFrame {
    let frame = boot_view_render_frame(bw, bh);
    er_d3d12_compositor::CompositorFrame {
        rgba: er_loading_bar_core::RgbaFrame {
            width: frame.w,
            height: frame.h,
            pixels: frame.rgba,
        },
        dst_x: frame.dx,
        dst_y: frame.dy,
    }
}

/// Everything `boot_view_rasterize` rasterizes into, named. Three consecutive `usize` geometry
/// pairs cannot be read safely at a positional call site, and there is no order the compiler
/// would have caught.
struct BootViewRaster {
    w: usize,
    h: usize,
    idx: usize,
    permille: usize,
    content_x: usize,
    content_y: usize,
    content_w: usize,
    text_scale: usize,
    draw_portrait: bool,
}

/// Rasterize either the original tight black progress strip, or a full-screen cached screenshot
/// background with the same understated bar/label geometry overlaid near the bottom.
fn boot_view_rasterize(
    spec: BootViewRaster,
    bg: Option<&er_boot_background::BootBgImage>,
) -> Vec<u8> {
    let BootViewRaster {
        w,
        h,
        idx,
        permille,
        content_x,
        content_y,
        content_w,
        text_scale,
        draw_portrait,
    } = spec;
    let mut buf = vec![0u8; w * h * RGBA8_BPP];
    let has_bg = bg.is_some();
    if let Some(bg) = bg {
        er_boot_background::boot_fill_aspect_cover_background(&mut buf, w, h, bg);
    } else {
        boot_fill_rect(&mut buf, w, h, 0, 0, w, h, BOOT_VIEW_RGB_BLACK);
    }
    // Character portrait BEHIND the bar/label: composite it right after the background so the bar, its
    // shadow band, and the phase label all draw in front (user 2026-07-15 "behind the loading bar").
    if draw_portrait {
        let _ = portrait_onto(&mut buf, w, h);
    }
    // Label = "<PHASE NAME> <i>/<N> (<SUBMILESTONE> <x>/<y>)". The main `i/N` is THIS epoch's phase
    // sequence, and the parenthesized subprogression belongs to the active phase: a phase with no known
    // finer RAM granularity says so with its own 1/1 substep, world-gauge phases use the native Gauge_3
    // frame, and entering-world uses only the handoff semaphores that are relevant once the world exists.
    let set = boot_view_phase_set();
    let (raw_sub_label, raw_sub_i, sub_max) = boot_view_phase_submilestone(set.phase(idx));
    // MONOTONIC display clamp (user 2026-07-19): within one load epoch the visible substep number must
    // only advance -- never repeat a passed value nor decrement. idx (main phase) is already monotonic;
    // clamp the substep to a per-phase high-water and hold the last-advanced label on a regression.
    let (sub_label, sub_i) = {
        let epoch = BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst);
        if BOOT_VIEW_MONO_EPOCH.swap(epoch, Ordering::SeqCst) != epoch {
            BOOT_VIEW_MONO_ORD.store(0, Ordering::SeqCst);
            BOOT_VIEW_MONO_LABEL_PTR.store(0, Ordering::SeqCst);
        }
        let ord = idx
            .saturating_mul(BOOT_VIEW_MONO_ORD_SCALE)
            .saturating_add(raw_sub_i.min(BOOT_VIEW_MONO_ORD_SCALE - 1));
        let hw = BOOT_VIEW_MONO_ORD.load(Ordering::SeqCst);
        if ord >= hw {
            BOOT_VIEW_MONO_ORD.store(ord, Ordering::SeqCst);
            BOOT_VIEW_MONO_LABEL_PTR.store(raw_sub_label.as_ptr() as usize, Ordering::SeqCst);
            BOOT_VIEW_MONO_LABEL_LEN.store(raw_sub_label.len(), Ordering::SeqCst);
            (raw_sub_label, raw_sub_i)
        } else if hw / BOOT_VIEW_MONO_ORD_SCALE == idx {
            // Regressed within the SAME phase -> hold the high-water substep + its (still-valid) label.
            let held_sub = hw % BOOT_VIEW_MONO_ORD_SCALE;
            let ptr = BOOT_VIEW_MONO_LABEL_PTR.load(Ordering::SeqCst);
            let len = BOOT_VIEW_MONO_LABEL_LEN.load(Ordering::SeqCst);
            let held: &str = if ptr != 0 {
                unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                        ptr as *const u8,
                        len,
                    ))
                }
            } else {
                raw_sub_label
            };
            (held, held_sub)
        } else {
            (raw_sub_label, raw_sub_i)
        }
    };
    // Surface the GameMan load-in-progress FSM (b80 == save_state) when a load is active (b80 > 0):
    // READING/RESIDENT. It stays visible through streaming and, notably, exposes the finalize-time
    // b80=RESIDENT stall (the case-7 blocker) directly on the bar. Hidden when idle (b80 <= 0).
    let b80 = SWITCH_ORACLE_B80.load(Ordering::SeqCst);
    let mms_step = SWITCH_ORACLE_MMS_STEP.load(Ordering::SeqCst);
    let finalize = SWITCH_ORACLE_FINALIZE_12A.load(Ordering::SeqCst);
    // The engine's live MoveMapStep step/finalize-substate name rides in the suffix, NOT in the main
    // `N/M`. It used to REPLACE the main phase label with a second, 22-long step sequence -- which both
    // changed the visible denominator mid-load and let a STALE in-world step from the previous epoch
    // read as 96% progress the instant a reload armed (bd er-effects-rs-ok8d). As a suffix it keeps its
    // whole diagnostic value (the bar still freezes on the exact stuck step by name during a softlock)
    // without pretending to be the phase sequence.
    // The MoveMapStep detail only means something once the world phases are running. Before then the
    // switch oracle still holds the PREVIOUS epoch's values, and showing them decorated an early reload
    // phase with a stale substate (`TITLE READY 2/8 (TITLE OWNER UP 1/1 - IDLE/DONE)`, measured run
    // samechar-3x-threedll-20260730-082930) -- the same stale-oracle leak this issue is about.
    let world_phase = matches!(
        set.phase(idx),
        er_loading_bar_core::LoadPhase::BuildingWorld
            | er_loading_bar_core::LoadPhase::StreamingWorld
            | er_loading_bar_core::LoadPhase::FinalizingWorld
            | er_loading_bar_core::LoadPhase::EnteringWorld
    );
    let load_suffix = if b80 > 0 {
        format!(" - SAVE {}", save_state_b80_name(b80))
    } else if !world_phase {
        String::new()
    } else if finalize >= 0 {
        format!(" - {}", movemapstep_finalize_substate_name(finalize))
    } else if mms_step != usize::MAX && mms_step < MOVEMAPSTEP_STEP_NAMES.len() {
        format!(" - {}", movemapstep_step_name(mms_step as i32))
    } else {
        String::new()
    };
    let label_model = er_loading_bar_core::LoadingLabel::new(
        set.label(idx),
        idx.min(set.main_total()),
        set.main_total(),
        sub_label,
        sub_i,
        sub_max,
    );
    let mut label_buf = String::new();
    label_model.write_text_with_sub_suffix(&mut label_buf, &load_suffix);
    let label: &str = &label_buf;
    let label_hash = boot_view_label_hash(label);
    if BOOT_VIEW_LAST_LABEL_HASH.swap(label_hash, Ordering::SeqCst) != label_hash {
        append_autoload_debug(format_args!("boot-view: label -> {label}"));
    }
    let strip_h = boot_view_strip_height(text_scale);
    let bar_y = content_y + BOOT_VIEW_GLYPH_H * text_scale + BOOT_VIEW_TEXT_BAR_GAP;
    if has_bg {
        // Local shadow band only around the UI, plus globally dimmed screenshot: keeps the hairline bar
        // readable on bright screenshots without turning the boot screen back into a heavy panel.
        er_boot_background::boot_darken_bar_shadow(
            &mut buf, w, h, content_x, content_y, content_w, strip_h,
        );
    }
    if has_bg {
        boot_draw_text_shadowed(&mut buf, w, h, content_x, content_y, label, text_scale);
    } else {
        boot_draw_text(&mut buf, w, h, content_x, content_y, label, text_scale);
    }
    // NO tick markers/labels (user 2026-07-15 "remove all of the markers ... remove other tick markers and
    // labels"): all phase information is carried by the single left-aligned granular label above the bar.
    boot_fill_rect(
        &mut buf,
        w,
        h,
        content_x,
        bar_y,
        content_w,
        BOOT_VIEW_BAR_H,
        BOOT_VIEW_RGB_TRACK,
    );
    boot_fill_rect(
        &mut buf,
        w,
        h,
        content_x,
        bar_y,
        content_w * permille.min(1000) / 1000,
        BOOT_VIEW_BAR_H,
        BOOT_VIEW_RGB_FILL,
    );
    buf
}

/// Draw one release-fade frame. The GPU half -- the command objects, the root signature/PSO/texture
/// slot and the alpha-blended full-screen draw -- moved to `er-cover-fade`, which has no product
/// decision in it. What stays here is everything that IS a product decision: taking the draw-busy
/// latch this file shares with the opaque composite path, rasterizing the frame from the current
/// strip geometry and progress reading, and this cover window's own bookkeeping on success.
unsafe fn composite_boot_release_fade_frame(swapchain_raw: usize, alpha: u8) -> bool {
    if BOOT_VIEW_DRAW_BUSY.swap(1, Ordering::SeqCst) != 0 {
        return false;
    }
    let _busy = BootViewBusyGuard;
    let mut rasterize = |w: usize, h: usize| -> Vec<u8> {
        let text_scale = boot_view_text_scale(h as u32);
        let strip_w = ((w as u32 * BOOT_VIEW_STRIP_W_NUM / BOOT_VIEW_STRIP_W_DEN)
            .max(BOOT_VIEW_STRIP_MIN_W)
            .min(w as u32)) as usize;
        let strip_h = boot_view_strip_height(text_scale).min(h);
        let strip_x = (w - strip_w) / 2;
        let strip_y = ((h as u32 * BOOT_VIEW_STRIP_Y_NUM / BOOT_VIEW_STRIP_Y_DEN) as usize)
            .min(h.saturating_sub(strip_h));
        let (ms_idx, permille) = boot_view_progress();
        boot_view_rasterize(
            BootViewRaster {
                w,
                h,
                idx: ms_idx,
                permille,
                content_x: strip_x,
                content_y: strip_y,
                content_w: strip_w,
                text_scale,
                draw_portrait: false,
            },
            None,
        )
    };
    if !unsafe { er_cover_fade::composite_release_fade_frame(swapchain_raw, alpha, &mut rasterize) }
    {
        return false;
    }
    boot_view_note_draw_after_stop("release-fade");
    BOOT_VIEW_FADE_HITS.fetch_add(1, Ordering::SeqCst);
    true
}

/// Composite the boot-progress strip onto the swapchain backbuffer. Called from the Present detour
/// for every pre-loading-window frame. This path MUST full-clear the backbuffer first: after the
/// self-present pump yields to Elden Ring's render loop, a strip-only copy lets the title/menu/world
/// render around the loading bar. Full-clear + strip copy keeps the bar persistent and tells the rest
/// of the frame, politely, to die in a fire.
pub(crate) unsafe fn composite_boot_progress_on_swapchain(
    _base: usize,
    swapchain_raw: usize,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        composite_boot_progress_inner(swapchain_raw, true, false)
    }))
    .unwrap_or(false)
}

/// Self-present-pump frame (pre-first-game-present): same draw, but the engine has NEVER rendered
/// this backbuffer, so its contents are undefined -- clear the whole RT to black before the strip
/// copy so no init-garbage flashes on screen.
pub(crate) unsafe fn composite_boot_progress_self_frame(swapchain_raw: usize) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        composite_boot_progress_inner(swapchain_raw, true, true)
    }))
    .unwrap_or(false)
}

/// RAII release of [`BOOT_VIEW_DRAW_BUSY`] on every exit path of the draw section.
struct BootViewBusyGuard;
impl Drop for BootViewBusyGuard {
    fn drop(&mut self) {
        BOOT_VIEW_DRAW_BUSY.store(0, Ordering::SeqCst);
    }
}

/// FPS-BAIL RESUME ON PUBLISH (bd er-effects-rs-dpf6 Phase 2). The permille arm of the FPS bail
/// latches on a HEALTHY fast switch load ~2s after confirm (measured run product-continue-direct-
/// 20260729-194759: bail at +136108 with permille=960, publish at +138063 -- the cover was dead 2.0s
/// before the head arrived). When a NEW portrait publish (version bump past the bail-time snapshot)
/// lands while the native loading screen is still active (update ticked / fadeout+close within the
/// same hold windows the release path uses), clear the bail stop ONCE per cover window so the head
/// composites for the remainder; the release fade / world handoff still owns the real end. Safe by
/// the frozen-load2 insight: a genuinely frozen load never publishes a portrait, so this can never
/// re-open the cover on the pathology the bail protects against. The 20s composite cap stays armed
/// post-resume as the FPS backstop (only the permille re-bail is suppressed).
fn boot_view_try_fps_bail_resume_on_publish() -> bool {
    if BOOT_VIEW_STOP_REASON.load(Ordering::SeqCst) != BOOT_VIEW_STOP_REASON_FPS_BAIL
        || BOOT_VIEW_FPS_BAIL_RESUMED.load(Ordering::SeqCst) != 0
    {
        return false;
    }
    let version = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    let bail_version = BOOT_VIEW_FPS_BAIL_PUBLISH_VERSION.load(Ordering::SeqCst);
    if version <= bail_version {
        return false;
    }
    let now_ms = boot_view_epoch_ms().max(1);
    let update_last = LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst) as u64;
    let fadeout_anchor = LOADING_SCREEN_GFX_FADEOUT_LAST_MS
        .load(Ordering::SeqCst)
        .max(LOADING_SCREEN_CLOSE_SENT_FIRST_MS.load(Ordering::SeqCst))
        as u64;
    let update_recent = update_last != 0
        && now_ms.saturating_sub(update_last) < BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS;
    let fadeout_recent = fadeout_anchor != 0
        && now_ms.saturating_sub(fadeout_anchor) < BOOT_VIEW_NATIVE_GFX_FADEOUT_HOLD_MS;
    if !(update_recent || fadeout_recent) {
        return false;
    }
    // Once per cover window (swap guards a concurrent Present racing this check).
    if BOOT_VIEW_FPS_BAIL_RESUMED.swap(1, Ordering::SeqCst) != 0 {
        return false;
    }
    // The bail cleared BOOT_VIEW_OWN_MENU_LOAD_ACTIVE; restore it so the resumed cover keeps the
    // own-menu stop semantics (table-baseline handoff + release fade) instead of first-boot ones.
    BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.store(
        BOOT_VIEW_FPS_BAIL_SLOT_KEY.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    BOOT_VIEW_STOP_REASON.store(0, Ordering::SeqCst);
    BOOT_VIEW_STOPPED.store(0, Ordering::SeqCst);
    // The window is drawing again, so the post-release watch must close with it -- otherwise a
    // resumed cover would keep sampling against a stop that has been taken back.
    er_telemetry_core::counters::BOOT_VIEW_STOP_MS.store(0, Ordering::SeqCst);
    let n = BOOT_VIEW_FPS_BAIL_RESUMES.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "boot-view: FPS-bail RESUME #{n} on portrait publish (version {bail_version} -> {version}, update_recent={update_recent} fadeout_recent={fadeout_recent}) -- compositing the published head for the rest of the window; release fade owns the end"
    ));
    true
}

unsafe fn composite_boot_progress_inner(
    swapchain_raw: usize,
    clear_first: bool,
    self_present_frame: bool,
) -> bool {
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) != 0 && !boot_view_try_fps_bail_resume_on_publish()
    {
        return false;
    }
    // HANDOFF: first start stops when the loading window / published keyed head / world takes over.
    // During an own-menu switch, the old-world and prior-keyed-frame latches are intentionally still
    // set, so stop only when THIS switch builds a fresh loading-screen table (baseline comparison).
    // NOTE: `now_loading_active` is deliberately NOT consulted: its `load_done` latch is false during
    // boot too, so it cannot distinguish "booting" from "loading".
    let own_menu_active = BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0;
    let loadscreen_builds = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst);
    let table_baseline = BOOT_VIEW_LOADSCREEN_TABLE_BASELINE.load(Ordering::SeqCst);
    let loading_handoff = if own_menu_active {
        loadscreen_builds > table_baseline
    } else {
        loadscreen_builds != 0 || PROFILE_HAVE_KEYED_FRAME.load(Ordering::SeqCst) != 0
    };
    // FPS FIX (bd fps-killer-rootcaused-per-frame-gpu-readback-boot-view-not-stopping-inworld-load2):
    // for the FIRST/boot load, IN_WORLD_REACHED (a one-shot latch) is the world-reached signal. For an
    // own-menu switch (load2+) that latch is STALE (already set from load1), so it can never stop the
    // per-frame GPU readback in-world -- the compositor kept readback-stalling the pipeline (~20-40fps).
    // Use the PER-EPOCH world-live signal instead: play_time advancing for the CURRENT fresh_deser epoch
    // means THIS switch's world is genuinely playable, so stop compositing (the loading cover is done).
    let cur_load_epoch =
        crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    let can_move_handoff = crate::constants::CAN_MOVE_CONFIRMED.load(Ordering::SeqCst)
        && crate::constants::MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == cur_load_epoch;
    let render_release_handoff = boot_view_cover_release_ready(can_move_handoff);
    let _epoch_world_handoff = render_release_handoff;
    let world_handoff = render_release_handoff;
    // ABSOLUTE BACKSTOP -- see [`boot_view_absolute_backstop`] for the whole argument. False in
    // every healthy window: while `world_handoff` is true the normal path owns the window and this
    // only samples, and its thresholds sit far past any measured legitimate load.
    //
    // It does NOT stop the cover itself. It routes the window down the SAME release fade a healthy
    // release uses, so the user gets the 640 ms fade rather than a hard pop, and the only thing it
    // overrides on the way is the native-loading hold below -- which cannot be evidence here,
    // because it is a recency read of counters a dead loading-screen detour never writes.
    let backstop = boot_view_absolute_backstop(boot_view_epoch_ms(), world_handoff);
    // DIAGNOSTIC (bd ab-portrait-disabled-load2-fps-still-low-boot-view-not-stopping): log the actual
    // stop gates. The product cover must not stop at player-present/native-loading; only the same
    // can_move epoch-gated proof used by the watcher is allowed to uncover the game.
    {
        let now_log = boot_view_epoch_ms();
        let last_log = BOOT_VIEW_DECISION_LOG_MS.load(Ordering::SeqCst);
        if now_log.saturating_sub(last_log) >= 1000
            && BOOT_VIEW_DECISION_LOG_MS
                .compare_exchange(last_log, now_log, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            let fresh_deser = crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT
                .load(Ordering::SeqCst);
            let move_epoch = crate::constants::MOVE_PROBE_EPOCH.load(Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "boot-view DECISION: own_menu={} loading_handoff={} world_handoff={} backstop={} backstop_world_live_ms={} can_move_handoff={} render_ready={} permille={} draw_state={} in_world={} fresh_deser={} move_epoch={} loadscreen_builds={} table_baseline={} now_ms={}",
                own_menu_active,
                loading_handoff,
                world_handoff,
                backstop,
                BOOT_VIEW_BACKSTOP_PT_RISE_LAST_MS
                    .load(Ordering::SeqCst)
                    .saturating_sub(BOOT_VIEW_BACKSTOP_PT_RISE_FIRST_MS.load(Ordering::SeqCst)),
                can_move_handoff,
                boot_view_player_render_ready(),
                BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
                BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst),
                IN_WORLD_REACHED.load(Ordering::SeqCst),
                fresh_deser,
                move_epoch,
                loadscreen_builds,
                table_baseline,
                now_log,
            ));
        }
    }
    let forced_continue_handoff = SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst)
        != 0
        || TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0
        || OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst)
        || OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.load(Ordering::SeqCst) != 0
        || TFC_FORCED_CONTINUE_HANDOFF_MS.load(Ordering::SeqCst) != 0;
    let native_loading_seen = LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst)
        > BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE.load(Ordering::SeqCst);
    let real_loading_handoff = forced_continue_handoff || native_loading_seen;
    if loading_handoff && !real_loading_handoff && !world_handoff {
        // Early profile/keyed-frame semaphores can assert while the game is still between title/menu
        // work and the forced Continue transition. They mean "keep covering", not "start the
        // handoff bail clock"; starting the clock here caused the cover to bail before CS::LoadingScreen
        // appeared, producing the user-visible black/loading gap.
        // Fall through and keep compositing.
    }
    if real_loading_handoff || world_handoff || backstop {
        // PRODUCT COVER (user 2026-07-25): native CS::LoadingScreen becoming lit is NOT a stop
        // condition. The product loading bar owns the full backbuffer until the game is actually
        // world/playable-ready. Native loading updates only prove the handoff happened and drive the
        // bar; the rest of the frame stays black-cleared.
        let now_ms = boot_view_epoch_ms().max(1) as usize;
        let mut seen_ms = BOOT_VIEW_HANDOFF_SEEN_MS.load(Ordering::SeqCst);
        if seen_ms == 0 {
            match BOOT_VIEW_HANDOFF_SEEN_MS.compare_exchange(
                0,
                now_ms,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    seen_ms = now_ms;
                    BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE.store(
                        LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst),
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "boot-view: handoff detected -> holding cover until native loading screen is lit (draws={} permille={} mask=0x{:x} own_menu={} table_builds={} table_baseline={})",
                        BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
                        BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
                        BOOT_VIEW_REACHED_MASK.load(Ordering::SeqCst),
                        own_menu_active,
                        loadscreen_builds,
                        table_baseline,
                    ));
                }
                Err(current) => seen_ms = current,
            }
        }
        let native_hits = LOADING_SCREEN_UPDATE_HITS
            .load(Ordering::SeqCst)
            .saturating_sub(BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE.load(Ordering::SeqCst));
        let held_ms = (now_ms as u64).saturating_sub(seen_ms as u64);
        let native_lit = native_hits >= BOOT_VIEW_NATIVE_LIT_UPDATE_HITS;
        if world_handoff || backstop {
            let native_gfx_fadeout_start =
                LOADING_SCREEN_GFX_FADEOUT_FIRST_MS.load(Ordering::SeqCst);
            let native_gfx_fadeout_last = LOADING_SCREEN_GFX_FADEOUT_LAST_MS.load(Ordering::SeqCst);
            let loading_update_last = LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst);
            let loading_close_ms = LOADING_SCREEN_CLOSE_SENT_FIRST_MS.load(Ordering::SeqCst);
            let fadeout_anchor = native_gfx_fadeout_last.max(loading_close_ms);
            let fadeout_pending = fadeout_anchor != 0
                && (now_ms as u64).saturating_sub(fadeout_anchor as u64)
                    < BOOT_VIEW_NATIVE_GFX_FADEOUT_HOLD_MS;
            let update_quiet_pending = loading_update_last != 0
                && (now_ms as u64).saturating_sub(loading_update_last as u64)
                    < BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS;
            // ONE-WAY RELEASE FADE (2026-08-22). Ask the hold as the START GATE it was written to
            // be. Once the fade has begun, only a `CS::LoadingScreen::Update` tick PAST the
            // fade-start snapshot can still hold. That used to be because the Scaleform half was
            // untrustworthy mid-fade -- it stamped on any movie's "fadeout" label, so opening the
            // in-world menu refreshed it. The narrowing on 2026-09-05 removed that reason, and the
            // rule stays anyway: what put the portrait back on screen was the FALL-THROUGH to the
            // opaque path, which is a bug about honouring a mid-fade hold at all, not about which
            // movie raised it. See [`boot_view_note_fade_hold_reassert`] for the whole argument.
            let start_gate_hold = fadeout_pending || update_quiet_pending;
            let fade_started = BOOT_VIEW_FADE_START_MS.load(Ordering::SeqCst) != 0;
            let native_gfx_hold_pending = if backstop {
                // NO HOLD SURVIVES THE BACKSTOP. Both halves of the hold are recency reads of
                // counters the CS::LoadingScreen detour writes, and a dead detour is the failure
                // this arrived to survive -- so an honest hold cannot exist here. Since 2026-09-05
                // a dishonest one cannot either: the Scaleform half only stamps for the loading
                // screen's own clip, which a dead detour never publishes. Kept regardless, because
                // the whole point of a backstop is not to depend on the thing that failed.
                false
            } else if fade_started {
                let ls_ticked_since_fade_start = LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst)
                    > BOOT_VIEW_FADE_START_LS_UPDATE_HITS.load(Ordering::SeqCst);
                let honored = ls_ticked_since_fade_start && update_quiet_pending;
                if start_gate_hold {
                    boot_view_note_fade_hold_reassert(
                        now_ms,
                        honored,
                        fadeout_pending,
                        update_quiet_pending,
                        ls_ticked_since_fade_start,
                    );
                } else {
                    BOOT_VIEW_FADE_HOLD_REASSERT_RUN.store(0, Ordering::SeqCst);
                }
                honored
            } else {
                start_gate_hold
            };
            if native_gfx_hold_pending && !fade_started {
                let hold_hits =
                    BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS.fetch_add(1, Ordering::SeqCst) + 1;
                if hold_hits <= 8 || hold_hits.is_power_of_two() {
                    append_autoload_debug(format_args!(
                        "boot-view: holding opaque cover through native loading fade/quiet window (fadeout_elapsed={}ms/{}, update_quiet={}ms/{}, native_hits={native_hits}, held_ms={held_ms}, render_ready={}, fadeout_hits={}, first_fadeout_ms={}, close_ms={}, draws={} permille={})",
                        (now_ms as u64).saturating_sub(fadeout_anchor as u64),
                        BOOT_VIEW_NATIVE_GFX_FADEOUT_HOLD_MS,
                        (now_ms as u64).saturating_sub(loading_update_last as u64),
                        BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS,
                        boot_view_player_render_ready(),
                        LOADING_SCREEN_GFX_FADEOUT_HITS.load(Ordering::SeqCst),
                        native_gfx_fadeout_start,
                        loading_close_ms,
                        BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
                        BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
                    ));
                }
                // Fall through to the normal full-clear + boot-bar path. The custom alpha fade is
                // deliberately delayed until native loading has both started its authored fade and stopped
                // updating long enough that the backbuffer behind our fade is gameplay, not loading art.
                //
                // Reachable only BEFORE the fade starts. A hold that arrives once the fade is
                // running never comes here: it pauses the fade below instead, because falling
                // through from mid-fade is what put the portrait back on screen at full alpha.
            } else if !native_gfx_hold_pending
                && BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS
                    .compare_exchange(0, now_ms, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                append_autoload_debug(format_args!(
                    "boot-view: native loading fade/quiet hold complete (hold_hits={}, fadeout_hits={}, first_fadeout_ms={}, last_fadeout_ms={}, update_last_ms={}, close_ms={})",
                    BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS.load(Ordering::SeqCst),
                    LOADING_SCREEN_GFX_FADEOUT_HITS.load(Ordering::SeqCst),
                    native_gfx_fadeout_start,
                    native_gfx_fadeout_last,
                    loading_update_last,
                    loading_close_ms,
                ));
            }
            // THE COMMITMENT. `fade_started` is enough on its own to enter: once the fade owns the
            // window a hold can pause it but can never send it back to the opaque path above.
            if !native_gfx_hold_pending || fade_started {
                let fade_start = match BOOT_VIEW_FADE_START_MS.compare_exchange(
                    0,
                    now_ms,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => {
                        BOOT_VIEW_STOP_NATIVE_HITS.store(native_hits, Ordering::SeqCst);
                        // The baseline every later hold is tested against. Taken here, in the same
                        // compare-exchange that decides which frame starts the fade, so exactly one
                        // frame writes it and it can never describe a different window.
                        BOOT_VIEW_FADE_START_LS_UPDATE_HITS.store(
                            LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst),
                            Ordering::SeqCst,
                        );
                        append_autoload_debug(format_args!(
                            "boot-view: world/playable handoff -> start release fade (native_hits={native_hits} held_ms={held_ms} native_lit={native_lit} native_gfx_fadeout_start_ms={native_gfx_fadeout_start} forced_continue={forced_continue_handoff} draws={} permille={})",
                            BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
                            BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
                        ));
                        now_ms
                    }
                    Err(start) => start,
                };
                // A honored hold PAUSES rather than cancels: the elapsed clock stops, so the alpha
                // freezes where it is and the full ramp still plays once the game's loading screen
                // goes quiet again. Capped, because the fade is this window's only exit.
                // Named apart from the outer `held_ms` (which is arm-to-now for the handoff log);
                // these are different clocks and shadowing them would be a trap for the next reader.
                let fade_held_ms = boot_view_fade_hold_tick(now_ms as u64, native_gfx_hold_pending);
                let fade_elapsed = (now_ms as u64)
                    .saturating_sub(fade_start as u64)
                    .saturating_sub(fade_held_ms);
                if fade_elapsed >= BOOT_VIEW_RELEASE_FADE_MS {
                    if BOOT_VIEW_STOPPED.swap(1, Ordering::SeqCst) == 0 {
                        BOOT_VIEW_FADE_COMPLETE_MS.store(now_ms, Ordering::SeqCst);
                        boot_view_stamp_stop_baselines(now_ms);
                        // Cover-window measurability (bd er-effects-rs-dpf6 Phase 1): stop reason
                        // (can-move world proof vs render-release) + arm->stop duration.
                        //
                        // The backstop is checked FIRST and unconditionally. A window that needed
                        // it was pathological long enough to trip it, and that stays the truth
                        // even if the healthy release became reachable during the fade it forced;
                        // filing such a window as reason 1 would hide the defect behind its own
                        // late recovery.
                        BOOT_VIEW_STOP_REASON.store(
                            if backstop {
                                BOOT_VIEW_STOP_REASON_ABSOLUTE_BACKSTOP
                            } else if can_move_handoff {
                                BOOT_VIEW_STOP_REASON_WORLD_HANDOFF
                            } else {
                                BOOT_VIEW_STOP_REASON_RELEASE_FADE
                            },
                            Ordering::SeqCst,
                        );
                        BOOT_VIEW_COVER_WINDOW_MS_LAST.store(
                            now_ms.saturating_sub(BOOT_VIEW_WINDOW_ARM_MS.load(Ordering::SeqCst)),
                            Ordering::SeqCst,
                        );
                        append_autoload_debug(format_args!(
                            "boot-view: release fade complete -> stop cover (fade_ms={fade_elapsed} fade_hits={} cover_window_ms={} reason={})",
                            BOOT_VIEW_FADE_HITS.load(Ordering::SeqCst),
                            BOOT_VIEW_COVER_WINDOW_MS_LAST.load(Ordering::SeqCst),
                            BOOT_VIEW_STOP_REASON.load(Ordering::SeqCst),
                        ));
                    }
                    BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.store(0, Ordering::SeqCst);
                    return false;
                }
                let remaining = BOOT_VIEW_RELEASE_FADE_MS.saturating_sub(fade_elapsed);
                let alpha = ((remaining * 255 + BOOT_VIEW_RELEASE_FADE_MS / 2)
                    / BOOT_VIEW_RELEASE_FADE_MS)
                    .clamp(1, 255) as u8;
                // MONOTONE. The pause above already keeps the ramp from brightening, so this is the
                // invariant stated rather than a second mechanism: from the first fade frame to the
                // last, the cover only ever gets more transparent. It is what makes "the portrait
                // cannot come back" a property of the code and not of the clock arithmetic -- and it
                // still holds on the frame the pause cap expires.
                let ceiling = BOOT_VIEW_FADE_LAST_ALPHA.load(Ordering::SeqCst);
                let alpha = if ceiling == 0 {
                    alpha
                } else {
                    alpha.min(ceiling.min(255) as u8)
                };
                BOOT_VIEW_FADE_LAST_ALPHA.store(alpha as usize, Ordering::SeqCst);
                if unsafe { composite_boot_release_fade_frame(swapchain_raw, alpha) } {
                    return true;
                }
                BOOT_VIEW_FADE_FAILURES.fetch_add(1, Ordering::SeqCst);
                return false;
            }
        }
        // Native loading exists; that is not permission to reveal the game. Fall through and keep
        // full-clearing + drawing the bar until the character render path is ready (or can-move proves
        // control on probe runs). If this stops early, the pre-world-stop/full-clear oracles fail.
    }
    // FPS BAIL (bd fps-killer-rootcaused-per-frame-gpu-readback-boot-view-not-stopping-inworld-load2):
    // when an own-menu reload STALLS at the finalize (frozen load2), it builds no new loadscreen table
    // and its play_time never advances, so NEITHER loading_handoff NOR world_handoff ever fires and the
    // per-frame GPU readback-with-wait above would run forever (~20fps). The loading bar itself still
    // fills (world resident/present at mms18), so stop the composite once the bar is essentially full OR
    // the composite has run past the per-epoch cap -- the readback must never permanently tank FPS.
    //
    // SCOPE, STATED SO IT IS NOT MISREAD AS GENERAL (2026-08-30): this is the OWN-MENU epoch's cap
    // and nothing else. The `if own_menu_active` below is the whole reason the boot epoch had no
    // bound at all, which is what [`boot_view_absolute_backstop`] now supplies for every epoch. The
    // 20 s cap here still fires first on a switch, so this path is unchanged in practice.
    if own_menu_active {
        let cur_epoch =
            crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        let now_ms = boot_view_epoch_ms().max(1);
        if crate::constants::BOOT_VIEW_COMPOSITE_EPOCH.swap(cur_epoch, Ordering::SeqCst)
            != cur_epoch
        {
            crate::constants::BOOT_VIEW_COMPOSITE_FIRST_MS.store(now_ms as usize, Ordering::SeqCst);
        }
        let first_ms = crate::constants::BOOT_VIEW_COMPOSITE_FIRST_MS.load(Ordering::SeqCst) as u64;
        let composite_ms = now_ms.saturating_sub(first_ms);
        let permille = BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst);
        // PERMILLE ARM REMOVED (er-effects-rs-drb7). It stopped the cover the moment the bar read
        // ~full, which on a HEALTHY switch happens ~1.3s in, long before the first portrait publish
        // -- a progress reading standing in for a freeze predicate. With no reachable semantic
        // release (see boot_view_cover_release_ready) this arm was the cover's de facto end
        // condition, and it produced the vanilla flash-through on every switch: run
        // slot-portrait-proof-20260731-115718 logged 4 bail stops, 0 semantic stops, 144 and 142
        // native-exposure frames with holes of 81 and 127 consecutive frames. The composite-time cap
        // stays and is now the bail's whole job: the guarantee that a genuinely frozen load can
        // never leave the per-frame GPU readback running forever.
        // Both still feed the stop log below -- they describe the state the cap fired in, they no
        // longer decide it.
        let resumed = BOOT_VIEW_FPS_BAIL_RESUMED.load(Ordering::SeqCst) != 0;
        if let Some(cap) = np::cap_fired(cur_epoch, now_ms, composite_ms, COMPOSITE_CAP) {
            if BOOT_VIEW_STOPPED.swap(1, Ordering::SeqCst) == 0 {
                // Phase-1/2 measurability: stop reason + window duration + the publish-version and
                // slot-key snapshots the publish-triggered resume compares/restores against.
                BOOT_VIEW_STOP_REASON.store(BOOT_VIEW_STOP_REASON_FPS_BAIL, Ordering::SeqCst);
                boot_view_stamp_stop_baselines(now_ms as usize);
                BOOT_VIEW_COVER_WINDOW_MS_LAST.store(
                    (now_ms as usize)
                        .saturating_sub(BOOT_VIEW_WINDOW_ARM_MS.load(Ordering::SeqCst)),
                    Ordering::SeqCst,
                );
                BOOT_VIEW_FPS_BAIL_PUBLISH_VERSION.store(
                    LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
                    Ordering::SeqCst,
                );
                BOOT_VIEW_FPS_BAIL_SLOT_KEY.store(
                    BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst),
                    Ordering::SeqCst,
                );
                append_autoload_debug(format_args!(
                    "boot-view: FPS BAIL stop (own-menu reload epoch={cur_epoch} permille={permille} composite_ms={composite_ms} resumed={resumed} cover_window_ms={}) {}; stopping per-frame GPU readback (resumable once on a fresh portrait publish while native loading is active)",
                    BOOT_VIEW_COVER_WINDOW_MS_LAST.load(Ordering::SeqCst),
                    np::bail_detail(&cap),
                ));
            }
            BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.store(0, Ordering::SeqCst);
            return false;
        }
    }
    if BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst) == 2 {
        return false;
    }
    // Mutual exclusion between the self-present pump thread and the game render thread (Present
    // detour): both use the same allocator/list/upload; the loser skips its frame.
    if BOOT_VIEW_DRAW_BUSY.swap(1, Ordering::SeqCst) != 0 {
        return false;
    }
    let _busy = BootViewBusyGuard;

    let sc_raw = swapchain_raw as *mut c_void;
    let Some(sc) = (unsafe { IDXGISwapChain3::from_raw_borrowed(&sc_raw) }) else {
        return false;
    };
    let idx = unsafe { sc.GetCurrentBackBufferIndex() };
    let Ok(backbuffer) = (unsafe { sc.GetBuffer::<ID3D12Resource>(idx) }) else {
        return false;
    };

    if BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst) == 0 {
        if unsafe { er_cover_fade::ensure_cover_command_objects(&backbuffer) } {
            BOOT_VIEW_DRAW_STATE.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("boot-view: draw state READY"));
        } else {
            BOOT_VIEW_DRAW_STATE.store(2, Ordering::SeqCst);
            append_autoload_debug(format_args!("boot-view: draw init FAILED -- giving up"));
            return false;
        }
    }

    let bb_desc = unsafe { backbuffer.GetDesc() };
    let bw = bb_desc.Width as u32;
    let bh = bb_desc.Height;
    if bw == 0 || bh == 0 || bw > MAX_RT_DIM || bh > MAX_RT_DIM {
        return false;
    }

    let picker_active = save_picker_overlay_active();
    let full_frame = boot_bg_image().is_some()
        || picker_active
        || stats_overlay_active()
        || portrait_overlay_active();
    let frame = boot_view_render_frame(bw as usize, bh as usize);
    let ms_idx = BOOT_VIEW_MILESTONE_IDX.load(Ordering::SeqCst);
    let permille = BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst);
    let geom_changed = BOOT_VIEW_STRIP_W.swap(frame.w, Ordering::SeqCst) != frame.w
        || BOOT_VIEW_STRIP_H.swap(frame.h, Ordering::SeqCst) != frame.h;
    if picker_active {
        SAVE_PICKER_OVERLAY_DRAW_HITS.fetch_add(1, Ordering::SeqCst);
    }
    BOOT_VIEW_DRAWN_PERMILLE.store(permille, Ordering::SeqCst);
    BOOT_VIEW_DRAWN_IDX.store(ms_idx, Ordering::SeqCst);
    BOOT_VIEW_DRAWN_BG_ACTIVE.store(full_frame as usize, Ordering::SeqCst);

    let rgba = er_loading_bar_core::RgbaFrame {
        width: frame.w,
        height: frame.h,
        pixels: frame.rgba,
    };
    if !unsafe {
        er_d3d12_compositor::copy_rgba_frame_to_swapchain(
            swapchain_raw,
            &rgba,
            frame.dx,
            frame.dy,
            clear_first,
        )
    } {
        if geom_changed {
            append_autoload_debug(format_args!(
                "boot-view: shared compositor copy failed after geometry change (region {}x{} at {},{} clear_first={})",
                rgba.width, rgba.height, frame.dx, frame.dy, clear_first as usize,
            ));
        }
        return false;
    }

    if clear_first {
        if self_present_frame {
            BOOT_VIEW_SELF_FULL_CLEAR_HITS.fetch_add(1, Ordering::SeqCst);
        } else {
            BOOT_VIEW_PRESENT_FULL_CLEAR_HITS.fetch_add(1, Ordering::SeqCst);
        }
    }

    boot_view_note_draw_after_stop("composite");
    boot_view_note_nonfade_draw_during_fade();
    let hits = BOOT_VIEW_DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "boot-view: first draw onto backbuffer {bw}x{bh} (region {}x{} at {},{}, bg={}, permille={permille})",
            rgba.width, rgba.height, frame.dx, frame.dy, full_frame as usize,
        ));
    }
    true
}
