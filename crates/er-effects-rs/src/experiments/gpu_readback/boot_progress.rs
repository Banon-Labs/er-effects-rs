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
pub(crate) use er_telemetry::counters::BOOT_VIEW_COVER_WINDOW_MS_LAST;
/// DIAGNOSTIC (bd ab-portrait-disabled-load2-fps-still-low-boot-view-composite-is-killer-2026-07-20):
/// last process-ms the per-frame boot-view stop DECISION was logged, so the log is rate-limited to
/// ~1/s while we diagnose why neither stop path fires for the incomplete load2.
pub(crate) use er_telemetry::counters::BOOT_VIEW_DECISION_LOG_MS;
/// Per-frame composite counter (RAM semaphore: the boot view is actually reaching the backbuffer).
pub(crate) use er_telemetry::counters::BOOT_VIEW_DRAW_HITS;
/// Draw-state machine: 0 = uninit, 1 = ready, 2 = failed (give up; never retry).
pub(crate) use er_telemetry::counters::BOOT_VIEW_DRAW_STATE;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FPS_BAIL_PUBLISH_VERSION;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FPS_BAIL_RESUMED;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FPS_BAIL_RESUMES;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FPS_BAIL_SLOT_KEY;
/// Last DISPLAYED progress in permille (monotonic; includes the inter-milestone creep).
pub(crate) use er_telemetry::counters::BOOT_VIEW_LAST_PERMILLE;
/// Baseline `PROFILE_LOADSCREEN_TABLE_BUILDS` when the own-menu switch rearmed the boot view; a later
/// increment is this switch's loading-window handoff. Default 0 preserves first-start behavior.
pub(crate) use er_telemetry::counters::BOOT_VIEW_LOADSCREEN_TABLE_BASELINE;
/// Monotonic-display clamp for the (phase idx, substep) LABEL numbers (user 2026-07-19): the visible
/// numbers must only advance within one load epoch -- never repeat a value already passed nor
/// decrement -- or the loading text reads as jumpy/looping. Ordinal = idx*ORD_SCALE + sub (phase idx
/// dominates). Held label is a `&'static str` (all sub-labels come from const tables, so its ptr/len
/// stay valid forever). Reset per load epoch.
pub(crate) use er_telemetry::counters::BOOT_VIEW_MONO_EPOCH;
pub(crate) use er_telemetry::counters::BOOT_VIEW_MONO_LABEL_LEN;
pub(crate) use er_telemetry::counters::BOOT_VIEW_MONO_LABEL_PTR;
pub(crate) use er_telemetry::counters::BOOT_VIEW_MONO_ORD;
/// Nonzero while the System->Quit custom ProfileSelect flow is switching to a picked slot. Value is
/// selected_slot + 1 so slot 0 is representable. This reopens the boot bar after the first world load.
pub(crate) use er_telemetry::counters::BOOT_VIEW_OWN_MENU_LOAD_ACTIVE;
pub(crate) use er_telemetry::counters::BOOT_VIEW_STOP_REASON;
/// One-shot stop latch: the loading window / world took over; reset only for a deliberate own-menu
/// character switch so the same custom progress bar can cover the return-title/autoload black gap.
pub(crate) use er_telemetry::counters::BOOT_VIEW_STOPPED;
pub(crate) use er_telemetry::counters::BOOT_VIEW_WINDOW_ARM_MS;
/// `BOOT_VIEW_STOP_REASON` values (0 = armed/none).
pub(crate) const BOOT_VIEW_STOP_REASON_RELEASE_FADE: usize = 1;
pub(crate) const BOOT_VIEW_STOP_REASON_FPS_BAIL: usize = 2;
pub(crate) const BOOT_VIEW_STOP_REASON_WORLD_HANDOFF: usize = 3;
const BOOT_VIEW_MONO_ORD_SCALE: usize = 1000;
/// Load-epoch identity + the per-epoch baselines for process-sticky counters (bd er-effects-rs-ok8d).
pub(crate) use er_telemetry::counters::BOOT_VIEW_CONTINUE_ALLOW_BASELINE;
pub(crate) use er_telemetry::counters::BOOT_VIEW_EPOCH_KIND;
pub(crate) use er_telemetry::counters::BOOT_VIEW_EPOCH_SEQ;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FRESH_DESER_BASELINE;
/// Hash of the last composed visible loading label logged to the runtime debug log.
pub(crate) use er_telemetry::counters::BOOT_VIEW_LAST_LABEL_HASH;
/// Highest reached milestone index (drives the label).
pub(crate) use er_telemetry::counters::BOOT_VIEW_MILESTONE_IDX;
pub(crate) use er_telemetry::counters::BOOT_VIEW_PORTRAIT_SPARED_BASELINE;
/// Monotonic bitmask of reached milestones (bit i = milestone i seen reached at least once).
pub(crate) use er_telemetry::counters::BOOT_VIEW_REACHED_MASK;
pub(crate) use er_telemetry::counters::BOOT_VIEW_TFC_CONTINUE_BASELINE;
/// `BOOT_VIEW_EPOCH_KIND` values.
const BOOT_VIEW_EPOCH_KIND_RELOAD: usize = 1;

// Our OWN persistent command objects (leaked raw pointers, same pattern as the portrait overlay --
// windows-rs COM types are !Send). Deliberately SEPARATE from the OVERLAY_* objects so the boot view
// cannot interfere with the proven portrait composite path or thrash its cached buffers at handoff.
pub(crate) use er_telemetry::counters::BOOT_VIEW_ALLOCATOR;
/// LOADING_SCREEN_UPDATE_HITS baseline latched at handoff detection: the counter is cumulative
/// across loads, so an own-menu second load must measure only ITS loading screen's ticks.
pub(crate) use er_telemetry::counters::BOOT_VIEW_DARK_GAP_FAILURES;
pub(crate) use er_telemetry::counters::BOOT_VIEW_DARK_GAP_LAST_HELD_MS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS;
/// Draw mutual-exclusion latch: the self-present pump thread and the game's render thread (Present
/// detour) share the command allocator/list; whoever loses the swap skips its frame.
pub(crate) use er_telemetry::counters::BOOT_VIEW_DRAW_BUSY;
/// 1 when the last rasterized upload included the optional cached screenshot background.
pub(crate) use er_telemetry::counters::BOOT_VIEW_DRAWN_BG_ACTIVE;
pub(crate) use er_telemetry::counters::BOOT_VIEW_DRAWN_IDX;
/// Last (permille, idx) actually rasterized into the upload buffer (skip the map/write when unchanged).
pub(crate) use er_telemetry::counters::BOOT_VIEW_DRAWN_PERMILLE;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FADE_COMPLETE_MS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FADE_FAILURES;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FADE_HITS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FADE_LAST_ALPHA;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FADE_START_MS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_FENCE;
pub(crate) use er_telemetry::counters::BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE;
/// Epoch-ms (never 0 once set) when the real loading/world handoff was first detected; the hold
/// clock for the seamless cut. Early profile/keyed-frame semaphores must not start this clock.
pub(crate) use er_telemetry::counters::BOOT_VIEW_HANDOFF_SEEN_MS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_LIST;
pub(crate) use er_telemetry::counters::BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_PRE_WORLD_STOP_FAILURES;
pub(crate) use er_telemetry::counters::BOOT_VIEW_PRESENT_COVER_FAILURES;
pub(crate) use er_telemetry::counters::BOOT_VIEW_PRESENT_FULL_CLEAR_HITS;
/// Why the self-present pump stopped: 0 = still running/never ran, 1 = game started presenting
/// (the goal), 2 = timeout budget, 3 = Present returned a failure HRESULT.
pub(crate) use er_telemetry::counters::BOOT_VIEW_PUMP_STOP_MS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_PUMP_STOP_REASON;
pub(crate) use er_telemetry::counters::BOOT_VIEW_QUEUE;
/// 1-descriptor RTV heap for the self-present full-clear (the engine has never rendered the
/// backbuffer before its first own present, so un-cleared regions would show garbage).
pub(crate) use er_telemetry::counters::BOOT_VIEW_RTV_HEAP;
pub(crate) use er_telemetry::counters::BOOT_VIEW_SELF_FULL_CLEAR_HITS;
/// Frames WE presented on the game's swapchain before its render loop produced its first frame.
pub(crate) use er_telemetry::counters::BOOT_VIEW_SELF_PRESENTS;
/// CS::LoadingScreen update hits at the moment the cover stopped (telemetry: proves the cut
/// happened on a lit loading screen, not into the black gap).
pub(crate) use er_telemetry::counters::BOOT_VIEW_STOP_NATIVE_HITS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_STRIP_H;
/// (w, h) the current upload buffer was rasterized for (strip geometry follows the backbuffer).
pub(crate) use er_telemetry::counters::BOOT_VIEW_STRIP_W;
/// Pump-relative ms at which the game swapchain was found + hooked (0 = never; pump path only).
pub(crate) use er_telemetry::counters::BOOT_VIEW_SWAPCHAIN_FOUND_MS;
pub(crate) use er_telemetry::counters::BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS;
/// Creep timing epoch + the epoch-ms when the milestone index last advanced.
static BOOT_VIEW_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
/// First-load latch: the initial game boot-to-title native loading-screen counters are sticky. Do not
/// let those stale "already reached 100%" semaphores drive the user-started load bar.
static BOOT_VIEW_FIRST_LOAD_REQUEST_REARMED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
pub(crate) use er_telemetry::counters::BOOT_VIEW_IDX_CHANGED_MS;

/// Optional, pre-decoded local screenshot background. This is intentionally disk-only: the DLL never
/// touches the network on the launch path. A helper script may populate this cache before launch.
struct BootBgImage {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

static BOOT_BG_IMAGE: std::sync::OnceLock<Option<BootBgImage>> = std::sync::OnceLock::new();

const BOOT_BG_CACHE_FILE: &str = "er-effects-boot-background.rgba";
const BOOT_BG_MAGIC: &[u8; 8] = b"ERBGRA01";
const BOOT_BG_STEAM_APPID: &str = "1245620";
const BOOT_BG_MAX_DIM: usize = 4096;
const BOOT_BG_MAX_PIXELS: usize = BOOT_BG_MAX_DIM * BOOT_BG_MAX_DIM;

/// The phase sequence THIS load epoch publishes. Boot walks engine bring-up through the first world;
/// a character reload cannot replay any of that, so it publishes a strictly smaller sequence and its
/// visible `N/M` denominator shrinks accordingly (bd er-effects-rs-ok8d).
fn boot_view_phase_set() -> &'static er_loading_bar::PhaseSet {
    if BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst) == BOOT_VIEW_EPOCH_KIND_RELOAD {
        &er_loading_bar::RELOAD_PHASE_SET
    } else {
        &er_loading_bar::BOOT_PHASE_SET
    }
}
/// Fill edge the bar pauses at while the startup save picker holds the boot (the MAIN MENU phase, whose
/// creep tops out near here). `boot_view_progress` clamps the fill here while a pick is pending, then
/// lifts it the frame the pick clears the latch.
const BOOT_VIEW_SAVE_CHECK_PERMILLE: usize = 470;
/// Asymptotic creep time-constant: creep = gap * since/(since + K). At `since == K` the bar is halfway to
/// the next milestone; it keeps approaching but never reaches it, so the bar NEVER fully freezes during a
/// long phase (user 2026-07-15: STARTING UP ~23s and the title load ~32s made a 70%-capped bar look stuck).
const BOOT_VIEW_CREEP_K_MS: u64 = 2600;
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

static BOOT_VIEW_FADE_ROOT_SIGNATURE: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_PSO: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_PSO_FORMAT: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_SRV_HEAP: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEXTURE: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_UPLOAD: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
static BOOT_VIEW_FADE_TEX_W: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEX_H: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEX_STATE: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEX_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX);

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
pub(crate) const BOOT_VIEW_GLYPH_H: usize = er_loading_bar::GLYPH_H;
/// Advance per character (5px glyph + 1px gap, pre-scale).
#[allow(dead_code)] // Retained: Glyph metric pair with the live BOOT_VIEW_GLYPH_H; kept so the two are read together.
pub(crate) const BOOT_VIEW_GLYPH_ADV: usize = er_loading_bar::GLYPH_ADV;
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
fn boot_phase_reached(phase: er_loading_bar::LoadPhase) -> bool {
    use er_loading_bar::LoadPhase as P;
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
fn boot_world_phase_reached(phase: er_loading_bar::LoadPhase) -> bool {
    use er_loading_bar::LoadPhase as P;
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
            let gauge_done = max_frame == 0 || cur_frame >= max_frame;
            progress >= 900 || (close_hits != 0 && gauge_done)
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
fn boot_view_cover_release_ready(can_move_handoff: bool) -> bool {
    use er_telemetry::counters::{
        BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN, BOOT_VIEW_RELEASE_READY_MS,
        BOOT_VIEW_RELEASE_RENDER_READY_SEEN, BOOT_VIEW_SEMANTIC_RELEASES,
    };
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
    if er_telemetry::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.load(Ordering::SeqCst) != 0 {
        let baseline =
            er_telemetry::counters::BOOT_VIEW_RELEASE_CONFIRM_BASELINE.load(Ordering::SeqCst);
        let fresh_deser =
            crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        if fresh_deser <= baseline {
            er_telemetry::counters::BOOT_VIEW_RELEASE_HELD_FOR_CONFIRM
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
    let player_loaded = unsafe { PlayerIns::local_player_mut() }
        .map(|player| {
            player.chr_ins.chr_model_ins.as_ptr() as usize != TITLE_OWNER_SCAN_START_ADDRESS
                && player.chr_ins.chr_ctrl.as_ptr() as usize != TITLE_OWNER_SCAN_START_ADDRESS
        })
        .unwrap_or(false);
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
    if LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst) != 0
        || LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst) >= 998
    {
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
            er_telemetry::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.load(Ordering::SeqCst);
        let baseline =
            er_telemetry::counters::BOOT_VIEW_RELEASE_CONFIRM_BASELINE.load(Ordering::SeqCst);
        let fresh_deser =
            crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        let before_confirm = require != 0 && fresh_deser <= baseline;
        if before_confirm {
            er_telemetry::counters::BOOT_VIEW_RELEASE_BEFORE_CONFIRM.fetch_add(1, Ordering::SeqCst);
        }
        append_autoload_debug(format_args!(
            "boot-view: COVER RELEASE #{n} at {now_ms}ms -- render-ready and native loading screen finishing both latched this window (the real handoff; no bail needed) require_confirm={require} fresh_deser={fresh_deser} baseline={baseline} before_confirm={before_confirm}"
        ));
    }
    ready
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
    LOADING_SCREEN_GFX_FADEOUT_FIRST_MS.store(0, Ordering::SeqCst);
    LOADING_SCREEN_GFX_FADEOUT_LAST_MS.store(0, Ordering::SeqCst);
}

/// Re-arm the cover DRAWING WINDOW: stop latch, window clock, handoff/fade/dark-gap state, draw cache.
/// This is "start covering again", independent of whether the phase walk is also starting over.
fn boot_view_reset_cover_window() {
    BOOT_VIEW_STOPPED.store(0, Ordering::SeqCst);
    BOOT_VIEW_STOP_REASON.store(0, Ordering::SeqCst);
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
    BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS.store(0, Ordering::SeqCst);
    BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_DARK_GAP_FAILURES.store(0, Ordering::SeqCst);
    BOOT_VIEW_DARK_GAP_LAST_HELD_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS.store(0, Ordering::SeqCst);
    // Release latches belong to the window that observed them.
    er_telemetry::counters::BOOT_VIEW_RELEASE_RENDER_READY_SEEN.store(0, Ordering::SeqCst);
    er_telemetry::counters::BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.store(0, Ordering::SeqCst);
    er_telemetry::counters::BOOT_VIEW_RELEASE_READY_MS.store(0, Ordering::SeqCst);
    // Default OFF: boot's single load has no teardown screen in front of it, so it must not wait
    // for a fresh-deser bump that will never come. `rearm_boot_progress_for_own_menu_load` turns it
    // back on after calling through here, which is the only path that faces two screens.
    er_telemetry::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.store(0, Ordering::SeqCst);
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
    er_telemetry::counters::BOOT_VIEW_RELEASE_CONFIRM_BASELINE.store(
        crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    er_telemetry::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.store(1, Ordering::SeqCst);
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
    if mask != prev_mask {
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
    // Asymptotic creep toward (never reaching) the next milestone, so a phase with no finer RAM
    // granularity still inches forward instead of freezing -- the "is it stuck?" fix.
    let since = now_ms.saturating_sub(BOOT_VIEW_IDX_CHANGED_MS.load(Ordering::SeqCst));
    let creep = (gap as u64 * since / (since + BOOT_VIEW_CREEP_K_MS)) as usize;
    let pm = sub_fill.max(base + creep).min(next).min(1000);
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
        .index_of(er_loading_bar::LoadPhase::BuildingWorld)
        .unwrap_or(0);
    let pm = if idx >= world_idx || native > 0 {
        let floor = set.base_permille_of(er_loading_bar::LoadPhase::BuildingWorld);
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
    use er_telemetry::counters as c;
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
/// single placeholder. One guaranteed-ordered RAM read: base + global -> +0x40 (u32 low dword).
/// The global staying null very early, or an out-of-range value, falls back to the coarse label so a
/// state number is never fabricated. RVA/offset/state names from the Ghidra 1.16.2 dump (CSSystemStep
/// ctor @0x140dec7c0, singleton @base+0x3d85680, current_state @+0x40) cross-checked against the
/// existing `oracle_system_step_label` telemetry; the Wait* states map to the ctor child steps
/// (res/file/pad/sound/graphics). InitBoot/WaitBoot pairs are the early core-boot sub-phases (each
/// InitBootN kicks the work, the paired WaitBootN blocks on it) -- named generically because their
/// per-index subsystem was not statically pinned. Labels are 5x7 font-safe (uppercase + digits).
const BOOT_SYS_STEP_GLOBAL_RVA: usize = er_game_base::rva::CS_SYSTEM_STEP_GLOBAL_RVA;
const BOOT_SYS_STEP_STATE_OFFSET: usize = 0x40;
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
            crate::experiments::safe_read_usize(base + BOOT_SYS_STEP_GLOBAL_RVA)
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
fn boot_view_phase_submilestone(phase: er_loading_bar::LoadPhase) -> (&'static str, usize, usize) {
    use er_loading_bar::LoadPhase as P;
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
    er_loading_bar::text_width(text, scale)
}

/// Blit `text` into the tight RGBA buffer at (x, y), scaled by `scale`.
// Argument-for-argument pass-through of `er_loading_bar::draw_text_rgb`. Grouping these into a
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
    er_loading_bar::draw_text_rgb(buf, w, h, x, y, text, rgb, scale);
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
// Argument-for-argument pass-through of `er_loading_bar::fill_rect_rgb`; same reasoning as
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
    er_loading_bar::fill_rect_rgb(buf, w, h, x0, y0, rw, rh, rgb);
}

fn boot_bg_image() -> Option<&'static BootBgImage> {
    BOOT_BG_IMAGE
        .get_or_init(|| {
            if let Some((path, img)) = boot_bg_toml_image_override() {
                append_autoload_debug(format_args!(
                    "boot-view: TOML background image loaded '{}' {}x{}",
                    path.display(),
                    img.width,
                    img.height
                ));
                return Some(img);
            }
            if let Some(img) = boot_bg_cache_override() {
                return Some(img);
            }
            if let Some((path, img)) = boot_bg_latest_local_steam_screenshot() {
                append_autoload_debug(format_args!(
                    "boot-view: local Steam screenshot background loaded '{}' {}x{}",
                    path.display(),
                    img.width,
                    img.height
                ));
                return Some(img);
            }
            None
        })
        .as_ref()
}

fn boot_bg_toml_image_override() -> Option<(std::path::PathBuf, BootBgImage)> {
    let path = crate::config::configured_boot_background_image()?;
    if !boot_bg_is_supported_image_path(&path) {
        append_autoload_debug(format_args!(
            "boot-view: TOML background image ignored '{}' (expected .jpg/.jpeg/.png file)",
            path.display()
        ));
        return None;
    }
    let img = unsafe { boot_bg_decode_wic_rgba(&path) }?;
    Some((path, img))
}

fn boot_bg_cache_override() -> Option<BootBgImage> {
    let path = game_directory_path()?.join(BOOT_BG_CACHE_FILE);
    let bytes = std::fs::read(&path).ok()?;
    match parse_boot_bg_cache(&bytes) {
        Some(img) => {
            append_autoload_debug(format_args!(
                "boot-view: cached screenshot background loaded '{}' {}x{}",
                path.display(),
                img.width,
                img.height
            ));
            Some(img)
        }
        None => {
            append_autoload_debug(format_args!(
                "boot-view: cached screenshot background ignored '{}' (bad ERBGRA01 cache)",
                path.display()
            ));
            None
        }
    }
}

fn parse_boot_bg_cache(bytes: &[u8]) -> Option<BootBgImage> {
    if bytes.len() < 16 || &bytes[..8] != BOOT_BG_MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    boot_bg_image_from_rgba(width, height, bytes[16..].to_vec())
}

fn boot_bg_image_from_rgba(width: usize, height: usize, rgba: Vec<u8>) -> Option<BootBgImage> {
    if width == 0 || height == 0 || width > BOOT_BG_MAX_DIM || height > BOOT_BG_MAX_DIM {
        return None;
    }
    let pixels = width.checked_mul(height)?;
    if pixels > BOOT_BG_MAX_PIXELS {
        return None;
    }
    let len = pixels.checked_mul(RGBA8_BPP)?;
    if rgba.len() != len {
        return None;
    }
    Some(BootBgImage {
        width,
        height,
        rgba,
    })
}

fn boot_bg_latest_local_steam_screenshot() -> Option<(std::path::PathBuf, BootBgImage)> {
    let path = boot_bg_find_latest_local_steam_screenshot()?;
    let img = unsafe { boot_bg_decode_wic_rgba(&path) }?;
    Some((path, img))
}

fn boot_bg_find_latest_local_steam_screenshot() -> Option<std::path::PathBuf> {
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for root in boot_bg_steam_userdata_roots() {
        let Ok(accounts) = std::fs::read_dir(&root) else {
            continue;
        };
        for account in accounts.flatten() {
            let account_path = account.path();
            if !account_path.is_dir() {
                continue;
            }
            let shots = account_path
                .join("760")
                .join("remote")
                .join(BOOT_BG_STEAM_APPID)
                .join("screenshots");
            let Ok(entries) = std::fs::read_dir(&shots) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !boot_bg_is_supported_image_path(&path) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let modified = meta
                    .modified()
                    .or_else(|_| meta.created())
                    .unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(t, _)| modified > *t) {
                    best = Some((modified, path));
                }
            }
        }
    }
    best.map(|(_, path)| path)
}

fn boot_bg_steam_userdata_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(game_dir) = game_directory_path() {
        for ancestor in game_dir.ancestors() {
            boot_bg_push_unique_root(&mut roots, ancestor.join("userdata"));
        }
    }
    for var in [
        "STEAM_COMPAT_CLIENT_INSTALL_PATH",
        "STEAM_HOME",
        "STEAM_ROOT",
    ] {
        if let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            boot_bg_push_unique_root(&mut roots, std::path::PathBuf::from(value).join("userdata"));
        }
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        let home = std::path::PathBuf::from(home);
        boot_bg_push_unique_root(
            &mut roots,
            home.join(".steam").join("steam").join("userdata"),
        );
        boot_bg_push_unique_root(
            &mut roots,
            home.join(".local")
                .join("share")
                .join("Steam")
                .join("userdata"),
        );
    }
    roots
}

fn boot_bg_push_unique_root(roots: &mut Vec<std::path::PathBuf>, path: std::path::PathBuf) {
    if roots.iter().any(|existing| existing == &path) {
        return;
    }
    roots.push(path);
}

fn boot_bg_is_supported_image_path(path: &std::path::Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "gif"
                )
            })
            .unwrap_or(false)
}

unsafe fn boot_bg_decode_wic_rgba(path: &std::path::Path) -> Option<BootBgImage> {
    // COM may already be initialized on this thread; ignore the HRESULT and let CoCreateInstance be
    // the real gate. WIC is local file decode only -- no network and no helper process.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let factory: IWICImagingFactory = unsafe {
        CoCreateInstance(
            &CLSID_WICImagingFactory,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?
    };
    let wide = boot_bg_wide_null(path);
    let decoder = unsafe {
        factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?
    };
    let frame = unsafe { decoder.GetFrame(0).ok()? };
    let source: IWICBitmapSource = frame.cast().ok()?;
    let converted = unsafe { WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &source).ok()? };
    let mut width = 0u32;
    let mut height = 0u32;
    unsafe { converted.GetSize(&mut width, &mut height).ok()? };
    let width_usize = width as usize;
    let height_usize = height as usize;
    let len = width_usize
        .checked_mul(height_usize)?
        .checked_mul(RGBA8_BPP)?;
    let mut rgba = vec![0u8; len];
    unsafe {
        converted
            .CopyPixels(std::ptr::null(), width * RGBA8_BPP as u32, &mut rgba)
            .ok()?
    };
    boot_bg_image_from_rgba(width_usize, height_usize, rgba)
}

fn boot_bg_wide_null(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn boot_fill_aspect_cover_background(buf: &mut [u8], w: usize, h: usize, bg: &BootBgImage) {
    // Integer aspect-cover mapping. The screenshot is deliberately dimmed so the loading bar remains
    // legible without adding a game-clashing panel. Keep this cheap: no launch-path blur/filter pass.
    let scale_by_width = w * bg.height >= h * bg.width;
    let (num, den) = if scale_by_width {
        (w, bg.width)
    } else {
        (h, bg.height)
    };
    let scaled_w = bg.width * num / den;
    let scaled_h = bg.height * num / den;
    let crop_x = scaled_w.saturating_sub(w) / 2;
    let crop_y = scaled_h.saturating_sub(h) / 2;
    for y in 0..h {
        let sy = ((y + crop_y) * den / num).min(bg.height - 1);
        for x in 0..w {
            let sx = ((x + crop_x) * den / num).min(bg.width - 1);
            let so = (sy * bg.width + sx) * RGBA8_BPP;
            let dofs = (y * w + x) * RGBA8_BPP;
            buf[dofs] = ((bg.rgba[so] as u16 * 6) / 16) as u8;
            buf[dofs + 1] = ((bg.rgba[so + 1] as u16 * 6) / 16) as u8;
            buf[dofs + 2] = ((bg.rgba[so + 2] as u16 * 6) / 16) as u8;
            buf[dofs + 3] = 255;
        }
    }
}

fn boot_darken_bar_shadow(
    buf: &mut [u8],
    w: usize,
    h: usize,
    content_x: usize,
    content_y: usize,
    content_w: usize,
    strip_h: usize,
) {
    // Soft vignette behind the progress UI: strongest at the bar center, fading to no darkening at
    // the edges. This keeps the hairline readable over bright screenshots without a hard rectangular
    // panel or a full-screen blur pass on the launch path.
    let x0 = content_x.saturating_sub(32);
    let y0 = content_y.saturating_sub(10);
    let rw = (content_w + 64).min(w.saturating_sub(x0));
    let rh = (strip_h + 20).min(h.saturating_sub(y0));
    if rw == 0 || rh == 0 {
        return;
    }
    let cx2 = (content_x * 2).saturating_add(content_w);
    let cy2 = (content_y * 2).saturating_add(strip_h);
    let rx = (rw.max(1) as u32).max(1);
    let ry = (rh.max(1) as u32).max(1);
    for y in y0..(y0 + rh).min(h) {
        let dy = ((y * 2).abs_diff(cy2) as u32).saturating_mul(255) / ry;
        for x in x0..(x0 + rw).min(w) {
            let dx = ((x * 2).abs_diff(cx2) as u32).saturating_mul(255) / rx;
            // Diamond-ish falloff: center -> strong shadow; edges -> original screenshot.
            let dist = ((dx + dy) / 2).min(255);
            let strength = 255u32.saturating_sub(dist);
            // Factor ranges roughly 3/8 at the center to 1.0 at the edge.
            let factor = 255u32.saturating_sub((strength * 5) / 8);
            let o = (y * w + x) * RGBA8_BPP;
            buf[o] = ((buf[o] as u32 * factor) / 255) as u8;
            buf[o + 1] = ((buf[o + 1] as u32 * factor) / 255) as u8;
            buf[o + 2] = ((buf[o + 2] as u32 * factor) / 255) as u8;
            buf[o + 3] = 255;
        }
    }
}

// BootViewFrame moved to er_loading_portrait::host (portrait crate split); the struct
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
        rgba: er_loading_bar::RgbaFrame {
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
fn boot_view_rasterize(spec: BootViewRaster, bg: Option<&BootBgImage>) -> Vec<u8> {
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
        boot_fill_aspect_cover_background(&mut buf, w, h, bg);
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
        er_loading_bar::LoadPhase::BuildingWorld
            | er_loading_bar::LoadPhase::StreamingWorld
            | er_loading_bar::LoadPhase::FinalizingWorld
            | er_loading_bar::LoadPhase::EnteringWorld
    );
    let load_suffix = if b80 > 0 {
        format!(" - SAVE {}", load_in_progress_b80_name(b80))
    } else if !world_phase {
        String::new()
    } else if finalize >= 0 {
        format!(" - {}", movemapstep_finalize_substate_name(finalize))
    } else if mms_step != usize::MAX && mms_step < MOVEMAPSTEP_STEP_NAMES.len() {
        format!(" - {}", movemapstep_step_name(mms_step as i32))
    } else {
        String::new()
    };
    let label_model = er_loading_bar::LoadingLabel::new(
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
        boot_darken_bar_shadow(&mut buf, w, h, content_x, content_y, content_w, strip_h);
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

/// One-time command-object init (device derived from the backbuffer; own DIRECT queue -- never the
/// game's). Mirrors the proven portrait-overlay init; separate objects on purpose.
unsafe fn boot_view_init(backbuffer: &ID3D12Resource) -> bool {
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    let Ok(allocator) = (unsafe {
        device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
    }) else {
        return false;
    };
    let Ok(list) = (unsafe {
        device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &allocator,
            None,
        )
    }) else {
        return false;
    };
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };
    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let Ok(queue) = (unsafe { device.CreateCommandQueue::<ID3D12CommandQueue>(&queue_desc) })
    else {
        return false;
    };
    let rtv_heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        NumDescriptors: 1,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
        NodeMask: 0,
    };
    let Ok(rtv_heap) =
        (unsafe { device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&rtv_heap_desc) })
    else {
        return false;
    };
    BOOT_VIEW_RTV_HEAP.store(rtv_heap.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_ALLOCATOR.store(allocator.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
    true
}

unsafe fn boot_view_fade_init(device: &ID3D12Device, format: DXGI_FORMAT, w: u32, h: u32) -> bool {
    let fmt = format.0 as usize;
    if BOOT_VIEW_FADE_ROOT_SIGNATURE.load(Ordering::SeqCst) != 0
        && BOOT_VIEW_FADE_PSO.load(Ordering::SeqCst) != 0
        && BOOT_VIEW_FADE_SRV_HEAP.load(Ordering::SeqCst) != 0
        && BOOT_VIEW_FADE_PSO_FORMAT.load(Ordering::SeqCst) == fmt
        && BOOT_VIEW_FADE_TEX_W.load(Ordering::SeqCst) == w as usize
        && BOOT_VIEW_FADE_TEX_H.load(Ordering::SeqCst) == h as usize
    {
        return true;
    }
    let Some(root_sig) = (unsafe { create_overlay_root_signature(device) }) else {
        return false;
    };
    let Some(pso) = (unsafe { create_overlay_pso(device, &root_sig, format) }) else {
        return false;
    };
    let srv_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
        NumDescriptors: 1,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
        NodeMask: 0,
    };
    let Ok(srv_heap) = (unsafe { device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&srv_desc) })
    else {
        return false;
    };
    if !unsafe {
        ensure_overlay_gpu_texture_slot(
            device,
            &srv_heap,
            w,
            h,
            0,
            &BOOT_VIEW_FADE_TEXTURE,
            &BOOT_VIEW_FADE_UPLOAD,
            &BOOT_VIEW_FADE_UPLOAD_SIZE,
            &BOOT_VIEW_FADE_TEX_W,
            &BOOT_VIEW_FADE_TEX_H,
            &BOOT_VIEW_FADE_TEX_STATE,
            &BOOT_VIEW_FADE_TEX_VERSION,
        )
    } {
        return false;
    }
    BOOT_VIEW_FADE_ROOT_SIGNATURE.store(root_sig.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FADE_PSO.store(pso.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FADE_SRV_HEAP.store(srv_heap.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FADE_PSO_FORMAT.store(fmt, Ordering::SeqCst);
    true
}

unsafe fn fill_boot_view_fade_upload(
    upload: &ID3D12Resource,
    alpha: u8,
    row_pitch: usize,
    w: usize,
    h: usize,
) -> bool {
    let total = BOOT_VIEW_FADE_UPLOAD_SIZE.load(Ordering::SeqCst) as usize;
    if total < row_pitch.saturating_mul(h) || row_pitch < w.saturating_mul(RGBA8_BPP) {
        return false;
    }
    let text_scale = boot_view_text_scale(h as u32);
    let strip_w = ((w as u32 * BOOT_VIEW_STRIP_W_NUM / BOOT_VIEW_STRIP_W_DEN)
        .max(BOOT_VIEW_STRIP_MIN_W)
        .min(w as u32)) as usize;
    let strip_h = boot_view_strip_height(text_scale).min(h);
    let strip_x = (w - strip_w) / 2;
    let strip_y = ((h as u32 * BOOT_VIEW_STRIP_Y_NUM / BOOT_VIEW_STRIP_Y_DEN) as usize)
        .min(h.saturating_sub(strip_h));
    let (ms_idx, permille) = boot_view_progress();
    let mut tight = boot_view_rasterize(
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
    );
    for px in tight.as_chunks_mut::<RGBA8_BPP>().0 {
        px[3] = alpha;
    }
    let mut map: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut map)) }.is_err() || map.is_null() {
        return false;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(map as *mut u8, total) };
    dst.fill(0);
    let src_row = w * RGBA8_BPP;
    for y in 0..h {
        let so = y * src_row;
        let dofs = y * row_pitch;
        if so + src_row > tight.len() || dofs + src_row > dst.len() {
            break;
        }
        dst[dofs..dofs + src_row].copy_from_slice(&tight[so..so + src_row]);
    }
    unsafe { upload.Unmap(0, None) };
    true
}

unsafe fn composite_boot_release_fade_frame(swapchain_raw: usize, alpha: u8) -> bool {
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
        if unsafe { boot_view_init(&backbuffer) } {
            BOOT_VIEW_DRAW_STATE.store(1, Ordering::SeqCst);
        } else {
            BOOT_VIEW_DRAW_STATE.store(2, Ordering::SeqCst);
            return false;
        }
    }
    if BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst) == 2 {
        return false;
    }
    let bb_desc = unsafe { backbuffer.GetDesc() };
    let cw = bb_desc.Width as u32;
    let ch = bb_desc.Height;
    if cw == 0 || ch == 0 || cw > MAX_RT_DIM || ch > MAX_RT_DIM {
        return false;
    }
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    if !unsafe { boot_view_fade_init(&device, bb_desc.Format, cw, ch) } {
        return false;
    }

    let tex_raw = BOOT_VIEW_FADE_TEXTURE.load(Ordering::SeqCst) as *mut c_void;
    let upload_raw = BOOT_VIEW_FADE_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
    let root_raw = BOOT_VIEW_FADE_ROOT_SIGNATURE.load(Ordering::SeqCst) as *mut c_void;
    let pso_raw = BOOT_VIEW_FADE_PSO.load(Ordering::SeqCst) as *mut c_void;
    let srv_heap_raw = BOOT_VIEW_FADE_SRV_HEAP.load(Ordering::SeqCst) as *mut c_void;
    let alloc_raw = BOOT_VIEW_ALLOCATOR.load(Ordering::SeqCst) as *mut c_void;
    let list_raw = BOOT_VIEW_LIST.load(Ordering::SeqCst) as *mut c_void;
    let fence_raw = BOOT_VIEW_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let queue_raw = BOOT_VIEW_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let rtv_heap_raw = BOOT_VIEW_RTV_HEAP.load(Ordering::SeqCst) as *mut c_void;
    let (
        Some(texture),
        Some(upload),
        Some(root_sig),
        Some(pso),
        Some(srv_heap),
        Some(allocator),
        Some(list),
        Some(fence),
        Some(queue),
        Some(rtv_heap),
    ) = (unsafe {
        (
            ID3D12Resource::from_raw_borrowed(&tex_raw),
            ID3D12Resource::from_raw_borrowed(&upload_raw),
            ID3D12RootSignature::from_raw_borrowed(&root_raw),
            ID3D12PipelineState::from_raw_borrowed(&pso_raw),
            ID3D12DescriptorHeap::from_raw_borrowed(&srv_heap_raw),
            ID3D12CommandAllocator::from_raw_borrowed(&alloc_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&list_raw),
            ID3D12Fence::from_raw_borrowed(&fence_raw),
            ID3D12CommandQueue::from_raw_borrowed(&queue_raw),
            ID3D12DescriptorHeap::from_raw_borrowed(&rtv_heap_raw),
        )
    })
    else {
        return false;
    };

    let desc = unsafe { texture.GetDesc() };
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    unsafe { device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, None) };
    if !unsafe {
        fill_boot_view_fade_upload(
            upload,
            alpha,
            footprint.Footprint.RowPitch as usize,
            cw as usize,
            ch as usize,
        )
    } {
        return false;
    }
    let rtv_cpu = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
    unsafe { device.CreateRenderTargetView(&backbuffer, None, rtv_cpu) };
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    if BOOT_VIEW_FADE_TEX_STATE.load(Ordering::SeqCst) == 1 {
        unsafe {
            record_transition(
                list,
                texture,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        };
        BOOT_VIEW_FADE_TEX_STATE.store(0, Ordering::SeqCst);
    }
    let mut src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(texture.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    unsafe { list.CopyTextureRegion(&dst, 0, 0, 0, &src, None) };
    unsafe { ManuallyDrop::drop(&mut src.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst.pResource) };
    unsafe {
        record_transition(
            list,
            texture,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        )
    };
    BOOT_VIEW_FADE_TEX_STATE.store(1, Ordering::SeqCst);
    unsafe {
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        )
    };
    let viewport = D3D12_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: cw as f32,
        Height: ch as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    let scissor = RECT {
        left: 0,
        top: 0,
        right: cw as i32,
        bottom: ch as i32,
    };
    let constants = [
        1.0f32.to_bits(),
        1.0f32.to_bits(),
        0.0f32.to_bits(),
        0.0f32.to_bits(),
    ];
    unsafe {
        list.SetGraphicsRootSignature(root_sig);
        list.SetPipelineState(pso);
        list.SetDescriptorHeaps(&[Some(srv_heap.clone())]);
        list.SetGraphicsRootDescriptorTable(0, srv_gpu_handle_at(&device, srv_heap, 0));
        list.SetGraphicsRoot32BitConstants(
            1,
            constants.len() as u32,
            constants.as_ptr() as *const c_void,
            0,
        );
        list.RSSetViewports(std::slice::from_ref(&viewport));
        list.RSSetScissorRects(std::slice::from_ref(&scissor));
        list.OMSetRenderTargets(1, Some(&rtv_cpu), true, None);
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.DrawInstanced(3, 1, 0, 0);
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_PRESENT,
        );
    }
    if !unsafe { execute_and_wait(queue, list, fence) } {
        return false;
    }
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
                "boot-view DECISION: own_menu={} loading_handoff={} world_handoff={} can_move_handoff={} render_ready={} permille={} draw_state={} in_world={} fresh_deser={} move_epoch={} loadscreen_builds={} table_baseline={} now_ms={}",
                own_menu_active,
                loading_handoff,
                world_handoff,
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
    if real_loading_handoff || world_handoff {
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
        if world_handoff {
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
            let native_gfx_hold_pending = fadeout_pending || update_quiet_pending;
            if native_gfx_hold_pending {
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
            } else if BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS
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
            if !native_gfx_hold_pending {
                let fade_start = match BOOT_VIEW_FADE_START_MS.compare_exchange(
                    0,
                    now_ms,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => {
                        BOOT_VIEW_STOP_NATIVE_HITS.store(native_hits, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "boot-view: world/playable handoff -> start release fade (native_hits={native_hits} held_ms={held_ms} native_lit={native_lit} native_gfx_fadeout_start_ms={native_gfx_fadeout_start} forced_continue={forced_continue_handoff} draws={} permille={})",
                            BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
                            BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
                        ));
                        now_ms
                    }
                    Err(start) => start,
                };
                let fade_elapsed = (now_ms as u64).saturating_sub(fade_start as u64);
                if fade_elapsed >= BOOT_VIEW_RELEASE_FADE_MS {
                    if BOOT_VIEW_STOPPED.swap(1, Ordering::SeqCst) == 0 {
                        BOOT_VIEW_FADE_COMPLETE_MS.store(now_ms, Ordering::SeqCst);
                        // Cover-window measurability (bd er-effects-rs-dpf6 Phase 1): stop reason
                        // (can-move world proof vs render-release) + arm->stop duration.
                        BOOT_VIEW_STOP_REASON.store(
                            if can_move_handoff {
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
        if composite_ms >= crate::constants::BOOT_VIEW_EPOCH_COMPOSITE_CAP_MS {
            if BOOT_VIEW_STOPPED.swap(1, Ordering::SeqCst) == 0 {
                // Phase-1/2 measurability: stop reason + window duration + the publish-version and
                // slot-key snapshots the publish-triggered resume compares/restores against.
                BOOT_VIEW_STOP_REASON.store(BOOT_VIEW_STOP_REASON_FPS_BAIL, Ordering::SeqCst);
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
                    "boot-view: FPS BAIL stop (own-menu reload epoch={cur_epoch} permille={permille} composite_ms={composite_ms} resumed={resumed} cover_window_ms={}) -- handoff signals never fired (frozen load2); stopping per-frame GPU readback (resumable once on a fresh portrait publish while native loading is active)",
                    BOOT_VIEW_COVER_WINDOW_MS_LAST.load(Ordering::SeqCst),
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
        if unsafe { boot_view_init(&backbuffer) } {
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

    let rgba = er_loading_bar::RgbaFrame {
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

    let hits = BOOT_VIEW_DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "boot-view: first draw onto backbuffer {bw}x{bh} (region {}x{} at {},{}, bg={}, permille={permille})",
            rgba.width, rgba.height, frame.dx, frame.dy, full_frame as usize,
        ));
    }
    true
}
