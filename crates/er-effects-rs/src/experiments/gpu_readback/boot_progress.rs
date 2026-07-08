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
//   LOADING  -- `PROFILE_LOADSCREEN_TABLE_BUILDS > 0` -> HANDOFF (stop; the loading-portrait
//               overlay/native Gauge_3 window owns the remaining progress from here)
//
// Reached milestones are latched into a monotonic bitmask (a latch that later reads 0 cannot walk
// the bar backwards), and the displayed value creeps part-way toward the next milestone over time so
// the bar visibly moves between semaphores. The draw is a single submit on our OWN queue (transition
// PRESENT->COPY_DEST, CopyTextureRegion upload->backbuffer strip rect, transition back, CPU fence
// wait) -- no backbuffer readback: the pre-Continue frames are the content-free black this view
// exists to replace, and the strip rect is entirely ours.

/// Draw-state machine: 0 = uninit, 1 = ready, 2 = failed (give up; never retry).
static BOOT_VIEW_DRAW_STATE: AtomicUsize = AtomicUsize::new(0);
/// One-shot stop latch: the loading window / world took over; reset only for a deliberate own-menu
/// character switch so the same custom progress bar can cover the return-title/autoload black gap.
static BOOT_VIEW_STOPPED: AtomicUsize = AtomicUsize::new(0);
/// Nonzero while the System->Quit custom ProfileSelect flow is switching to a picked slot. Value is
/// selected_slot + 1 so slot 0 is representable. This reopens the boot bar after the first world load.
pub(crate) static BOOT_VIEW_OWN_MENU_LOAD_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Baseline `PROFILE_LOADSCREEN_TABLE_BUILDS` when the own-menu switch rearmed the boot view; a later
/// increment is this switch's loading-window handoff. Default 0 preserves first-start behavior.
pub(crate) static BOOT_VIEW_LOADSCREEN_TABLE_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Per-frame composite counter (RAM semaphore: the boot view is actually reaching the backbuffer).
pub(crate) static BOOT_VIEW_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
/// Last DISPLAYED progress in permille (monotonic; includes the inter-milestone creep).
pub(crate) static BOOT_VIEW_LAST_PERMILLE: AtomicUsize = AtomicUsize::new(0);
/// Monotonic bitmask of reached milestones (bit i = milestone i seen reached at least once).
pub(crate) static BOOT_VIEW_REACHED_MASK: AtomicUsize = AtomicUsize::new(0);
/// Highest reached milestone index (drives the label).
pub(crate) static BOOT_VIEW_MILESTONE_IDX: AtomicUsize = AtomicUsize::new(0);

// Our OWN persistent command objects (leaked raw pointers, same pattern as the portrait overlay --
// windows-rs COM types are !Send). Deliberately SEPARATE from the OVERLAY_* objects so the boot view
// cannot interfere with the proven portrait composite path or thrash its cached buffers at handoff.
static BOOT_VIEW_ALLOCATOR: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_LIST: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FENCE: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_QUEUE: AtomicUsize = AtomicUsize::new(0);
/// Persistent UPLOAD buffer holding the rasterized strip (recreated when the footprint changes).
static BOOT_VIEW_UPLOAD: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
/// 1-descriptor RTV heap for the self-present full-clear (the engine has never rendered the
/// backbuffer before its first own present, so un-cleared regions would show garbage).
static BOOT_VIEW_RTV_HEAP: AtomicUsize = AtomicUsize::new(0);
/// Draw mutual-exclusion latch: the self-present pump thread and the game's render thread (Present
/// detour) share the command allocator/list; whoever loses the swap skips its frame.
static BOOT_VIEW_DRAW_BUSY: AtomicUsize = AtomicUsize::new(0);
/// Frames WE presented on the game's swapchain before its render loop produced its first frame.
pub(crate) static BOOT_VIEW_SELF_PRESENTS: AtomicUsize = AtomicUsize::new(0);
/// Pump-relative ms at which the game swapchain was found + hooked (0 = never; pump path only).
pub(crate) static BOOT_VIEW_SWAPCHAIN_FOUND_MS: AtomicUsize = AtomicUsize::new(0);
/// Why the self-present pump stopped: 0 = still running/never ran, 1 = game started presenting
/// (the goal), 2 = timeout budget, 3 = Present returned a failure HRESULT.
pub(crate) static BOOT_VIEW_PUMP_STOP_REASON: AtomicUsize = AtomicUsize::new(0);
/// (w, h) the current upload buffer was rasterized for (strip geometry follows the backbuffer).
static BOOT_VIEW_STRIP_W: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_STRIP_H: AtomicUsize = AtomicUsize::new(0);
/// Last (permille, idx) actually rasterized into the upload buffer (skip the map/write when unchanged).
static BOOT_VIEW_DRAWN_PERMILLE: AtomicUsize = AtomicUsize::new(usize::MAX);
static BOOT_VIEW_DRAWN_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
/// 1 when the last rasterized upload included the optional cached screenshot background.
static BOOT_VIEW_DRAWN_BG_ACTIVE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Epoch-ms (never 0 once set) when the loading/world handoff was first detected; the hold clock
/// for the seamless cut. Reset by an own-menu rearm.
pub(crate) static BOOT_VIEW_HANDOFF_SEEN_MS: AtomicUsize = AtomicUsize::new(0);
/// CS::LoadingScreen update hits at the moment the cover stopped (telemetry: proves the cut
/// happened on a lit loading screen, not into the black gap).
pub(crate) static BOOT_VIEW_STOP_NATIVE_HITS: AtomicUsize = AtomicUsize::new(0);
/// LOADING_SCREEN_UPDATE_HITS baseline latched at handoff detection: the counter is cumulative
/// across loads, so an own-menu second load must measure only ITS loading screen's ticks.
pub(crate) static BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Creep timing epoch + the epoch-ms when the milestone index last advanced.
static BOOT_VIEW_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
static BOOT_VIEW_IDX_CHANGED_MS: AtomicU64 = AtomicU64::new(0);

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

/// Milestone labels (5x7 font glyph coverage: A-Z subset + digits + '%'; see `boot_glyph_5x7`).
const BOOT_VIEW_MILESTONE_LABELS: [&str; 7] = [
    "BOOT", "GAME", "OFFLINE", "TITLE", "MENU", "CONTINUE", "LOADING",
];
/// Progress targets per milestone, in permille. Spacing follows the measured product-run timeline
/// (first present +3.5s, offline +8.5s, title/menu ~+10s, continue ~+15s, table builds ~+15.5s) so
/// the bar's pace roughly matches wall-clock without ever depending on it. The final pre-loading-view
/// milestone deliberately stops at the native-handoff marker instead of 100%: the remaining gap is owned
/// by the game's real now-loading Gauge_3 bar, whose terminal frame is the true all-loading-complete
/// semaphore.
// SAVE CHECK sits at the fill edge where the bar actually PAUSES while the missing-save overlay
// picker is up. The native title menu-open is now HELD until the pick (title_open_menu_suppress_hook),
// so the MENU milestone (490) cannot latch while the picker is pending -- the bar stalls at the TITLE
// milestone (385) and its inter-milestone creep tops out at 385 + 7/10*(490-385) = 458. The clamp in
// `boot_view_progress` pins the fill edge exactly here while the pick is pending; it lifts the frame the
// pick clears the latch, so the bar resumes MENU -> CONTINUE -> SAVE LOAD / NATIVE. (Was 570, which
// matched the OLD flow where the menu opened before the pick and the bar paused at the MENU milestone.)
const BOOT_VIEW_SAVE_CHECK_PERMILLE: usize = 458;
const BOOT_VIEW_SAVE_CHECK_LABEL: &str = "SAVE CHECK";
const BOOT_VIEW_SAVE_LOAD_PERMILLE: usize = 615;
const BOOT_VIEW_SAVE_LOAD_LABEL: &str = "SAVE LOAD";
const BOOT_VIEW_NATIVE_HANDOFF_PERMILLE: usize = 700;
const BOOT_VIEW_NATIVE_HANDOFF_LABEL: &str = "NATIVE";
const BOOT_VIEW_HANDOFF_MARKER_W: usize = 3;
const BOOT_VIEW_HANDOFF_GAP_W: usize = 9;
const BOOT_VIEW_MILESTONE_PERMILLE: [usize; 7] = [
    45,
    140,
    245,
    385,
    490,
    615,
    BOOT_VIEW_NATIVE_HANDOFF_PERMILLE,
];
/// Inter-milestone creep: over this window the bar moves up to 7/10 of the gap to the next target.
const BOOT_VIEW_CREEP_FULL_MS: u64 = 6000;
const BOOT_VIEW_CREEP_NUM: usize = 7;
const BOOT_VIEW_CREEP_DEN: usize = 10;
/// Seamless handoff (user 2026-07-06, replacing the earlier fade-out design): at the loading
/// handoff the cover HOLDS fully lit over the game's black gap and the loading screen's own
/// fade-in-from-black, then stops in a single cut once the native loading screen is fully lit --
/// a lit-to-lit scene cut with no black and no fade. Measured (run 194254 pixel telemetry): the
/// native fade-in luminance plateaus around CS::LoadingScreen update hit ~12, ~1.8s after the
/// loading-table build.
const BOOT_VIEW_NATIVE_LIT_UPDATE_HITS: usize = 12;
/// If the CS::LoadingScreen update semaphore never advances (hook missing/regressed), stop this
/// long after the handoff anyway so the cover can never mask the live loading screen indefinitely.
const BOOT_VIEW_HANDOFF_HOLD_BAIL_MS: u64 = 5_000;

// Strip geometry (pixels; text is the 5x7 font at 2x = 10x14). ER-idiomatic minimal presentation
// (user 2026-07-05: the panel/border/percent styling clashed with the game): a hairline bar on a
// dark track near the bottom of the screen -- the game's own now-loading bar language -- with a
// small dim label above it. Everything else in the copied strip rect is pure black, which is
// indistinguishable from the black boot frames underneath, so only the bar + label are visible.
// (The game's REAL loading-bar widget/asset cannot be reused here: its menu resources are not in
// game memory until ~+12.7s and the DLL must not unpack assets from disk itself.)
const BOOT_VIEW_TEXT_SCALE: usize = 2;
const BOOT_VIEW_GLYPH_W: usize = 5;
const BOOT_VIEW_GLYPH_H: usize = 7;
/// Advance per character (5px glyph + 1px gap, pre-scale).
const BOOT_VIEW_GLYPH_ADV: usize = 6;
/// Hairline bar, like the game's own loading bar.
const BOOT_VIEW_BAR_H: usize = 3;
/// Gap between the text row and the bar track.
const BOOT_VIEW_TEXT_BAR_GAP: usize = 5;
/// Bottom padding row so the handoff marker never touches the strip edge.
const BOOT_VIEW_PAD_BOTTOM: usize = 3;
/// Total strip height: text row, gap, bar, bottom pad.
const BOOT_VIEW_STRIP_HEIGHT: usize = BOOT_VIEW_GLYPH_H * BOOT_VIEW_TEXT_SCALE
    + BOOT_VIEW_TEXT_BAR_GAP
    + BOOT_VIEW_BAR_H
    + BOOT_VIEW_PAD_BOTTOM;
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

/// True once milestone `idx`'s semaphore has asserted. Every predicate is a pure atomic/pointer read
/// that is safe from the render thread; ordering mistakes degrade to a stalled bar, never a lie about
/// sequence (the reached MASK is latched monotonic by the caller).
fn boot_milestone_reached(idx: usize) -> bool {
    if BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0 {
        let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
        return match idx {
            // Drawing at all proves the present hook + game swapchain are live.
            0 => true,
            1 => game_man_ptr_or_null() != 0,
            // Offline bytes are already cleared by the first boot in the same process.
            2 => FORCE_OFFLINE_BYTES_CLEARED.load(Ordering::SeqCst) != 0,
            // Own-menu switch phases replace stale first-boot title/menu latches.
            3 => phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
            4 => phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN,
            5 => phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
                || SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst) != 0
                || TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0,
            6 => {
                PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst)
                    > BOOT_VIEW_LOADSCREEN_TABLE_BASELINE.load(Ordering::SeqCst)
            }
            _ => false,
        };
    }
    match idx {
        // Drawing at all proves the present hook + game swapchain are live.
        0 => true,
        1 => game_man_ptr_or_null() != 0,
        2 => FORCE_OFFLINE_BYTES_CLEARED.load(Ordering::SeqCst) != 0,
        3 => TITLE_FADEIN_SKIP_FIRED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS,
        // Menu-open era: the own-stepper latch when that task runs, OR'd with the network-check
        // shortcircuit which fires ~10ms after the title-accept-byte natural menu-open on the
        // product path (runtime-proven 2026-07-05: latch stayed 0, shortcircuit fired at +12.8s).
        4 => {
            PRODUCT_CORE_LAST_MENU_OPENED_LATCH.load(Ordering::SeqCst) != 0
                || NETWORK_CHECK_SHORTCIRCUIT_COUNT.load(Ordering::SeqCst) != 0
        }
        // Continue committed: the confirm/TFC counters on their paths, OR'd with the portrait
        // teardown-SPARE which lands in the same millisecond as the Continue SetState5 on the
        // portrait-lookat product path (runtime-proven 2026-07-05: counters stayed 0, spare fired).
        5 => {
            SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst) != 0
                || TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0
                || LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) != 0
        }
        6 => {
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst)
                > BOOT_VIEW_LOADSCREEN_TABLE_BASELINE.load(Ordering::SeqCst)
        }
        _ => false,
    }
}

/// Compute the current (milestone idx, displayed permille). Latches newly reached milestones into the
/// monotonic mask, stamps idx-change time for the creep, and never lets the displayed value decrease.
fn boot_view_epoch_ms() -> u64 {
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
    BOOT_VIEW_STOPPED.store(0, Ordering::SeqCst);
    BOOT_VIEW_HANDOFF_SEEN_MS.store(0, Ordering::SeqCst);
    BOOT_VIEW_STOP_NATIVE_HITS.store(0, Ordering::SeqCst);
    BOOT_VIEW_REACHED_MASK.store(1, Ordering::SeqCst);
    BOOT_VIEW_MILESTONE_IDX.store(0, Ordering::SeqCst);
    BOOT_VIEW_LAST_PERMILLE.store(0, Ordering::SeqCst);
    BOOT_VIEW_DRAWN_PERMILLE.store(usize::MAX, Ordering::SeqCst);
    BOOT_VIEW_DRAWN_IDX.store(usize::MAX, Ordering::SeqCst);
    BOOT_VIEW_DRAWN_BG_ACTIVE.store(usize::MAX, Ordering::SeqCst);
    BOOT_VIEW_IDX_CHANGED_MS.store(boot_view_epoch_ms(), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "boot-view: rearmed for own-menu character load selected_slot={selected_slot} source={source} table_baseline={table_baseline}"
    ));
}

fn boot_view_progress() -> (usize, usize) {
    let mut mask = BOOT_VIEW_REACHED_MASK.load(Ordering::SeqCst);
    for i in 0..BOOT_VIEW_MILESTONE_LABELS.len() {
        if mask & (1 << i) == 0 && boot_milestone_reached(i) {
            mask |= 1 << i;
        }
    }
    BOOT_VIEW_REACHED_MASK.store(mask, Ordering::SeqCst);
    let idx = (usize::BITS - 1 - mask.max(1).leading_zeros()) as usize;
    let now_ms = boot_view_epoch_ms();
    let prev_idx = BOOT_VIEW_MILESTONE_IDX.swap(idx, Ordering::SeqCst);
    if prev_idx != idx {
        BOOT_VIEW_IDX_CHANGED_MS.store(now_ms, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "boot-view: milestone -> {} (idx {idx}, mask 0x{mask:x})",
            BOOT_VIEW_MILESTONE_LABELS[idx]
        ));
    }
    let base = BOOT_VIEW_MILESTONE_PERMILLE[idx.min(BOOT_VIEW_MILESTONE_PERMILLE.len() - 1)];
    let next = if idx + 1 < BOOT_VIEW_MILESTONE_PERMILLE.len() {
        BOOT_VIEW_MILESTONE_PERMILLE[idx + 1]
    } else {
        base
    };
    let since = now_ms.saturating_sub(BOOT_VIEW_IDX_CHANGED_MS.load(Ordering::SeqCst));
    let creep = (next.saturating_sub(base) * (since.min(BOOT_VIEW_CREEP_FULL_MS) as usize)
        * BOOT_VIEW_CREEP_NUM)
        / (BOOT_VIEW_CREEP_FULL_MS as usize * BOOT_VIEW_CREEP_DEN);
    let pm = (base + creep).min(1000);
    // While the overlay picker holds the boot, clamp the fill so its edge stops EXACTLY at the
    // SAVE CHECK tick (the MENU milestone + creep would otherwise creep ~7 permille past it, leaving
    // the tick sitting behind the fill edge). The clamp lifts the frame the pick clears the latch,
    // so the bar resumes past SAVE CHECK toward SAVE LOAD / NATIVE.
    let pm = if missing_save_selection_pending() {
        pm.min(BOOT_VIEW_SAVE_CHECK_PERMILLE)
    } else {
        pm
    };
    // Monotonic display: an idx re-latch or timer wobble must never walk the bar backwards.
    let shown = BOOT_VIEW_LAST_PERMILLE.fetch_max(pm, Ordering::SeqCst).max(pm);
    (idx, shown)
}

/// 5x7 glyphs for the milestone labels + percent readout. Each row byte uses bit 4 as the LEFTMOST
/// pixel. Hand-authored for this module (our own asset; nothing game-derived). Unknown chars render
/// as blanks rather than failing.
fn boot_glyph_5x7(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0e],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '\\' => [0x10, 0x08, 0x08, 0x04, 0x02, 0x02, 0x01],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        '[' => [0x0e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0e],
        ']' => [0x0e, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0e],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '>' => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
        '?' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1f],
        '3' => [0x0e, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        '%' => [0x19, 0x19, 0x02, 0x04, 0x08, 0x13, 0x13],
        _ => [0; 7],
    }
}

fn boot_text_width(text: &str) -> usize {
    text.chars().count() * BOOT_VIEW_GLYPH_ADV * BOOT_VIEW_TEXT_SCALE
}

/// Blit `text` into the tight RGBA buffer at (x, y), scaled by `BOOT_VIEW_TEXT_SCALE`.
fn boot_draw_text_rgb(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    text: &str,
    rgb: [u8; 3],
) {
    let mut cx = x;
    for c in text.chars() {
        let rows = boot_glyph_5x7(c);
        for (gy, row) in rows.iter().enumerate() {
            for gx in 0..BOOT_VIEW_GLYPH_W {
                if row & (1 << (BOOT_VIEW_GLYPH_W - 1 - gx)) == 0 {
                    continue;
                }
                for sy in 0..BOOT_VIEW_TEXT_SCALE {
                    for sx in 0..BOOT_VIEW_TEXT_SCALE {
                        let px = cx + gx * BOOT_VIEW_TEXT_SCALE + sx;
                        let py = y + gy * BOOT_VIEW_TEXT_SCALE + sy;
                        if px < w && py < h {
                            let o = (py * w + px) * RGBA8_BPP;
                            buf[o] = rgb[0];
                            buf[o + 1] = rgb[1];
                            buf[o + 2] = rgb[2];
                            buf[o + 3] = 255;
                        }
                    }
                }
            }
        }
        cx += BOOT_VIEW_GLYPH_ADV * BOOT_VIEW_TEXT_SCALE;
    }
}

fn boot_draw_text(buf: &mut [u8], w: usize, h: usize, x: usize, y: usize, text: &str) {
    boot_draw_text_rgb(buf, w, h, x, y, text, BOOT_VIEW_RGB_TEXT);
}

fn boot_draw_text_shadowed(buf: &mut [u8], w: usize, h: usize, x: usize, y: usize, text: &str) {
    boot_draw_text_rgb(
        buf,
        w,
        h,
        x.saturating_add(2),
        y.saturating_add(2),
        text,
        BOOT_VIEW_RGB_BLACK,
    );
    boot_draw_text(buf, w, h, x, y, text);
}

/// Axis-aligned opaque fill into the tight RGBA buffer (clamped).
fn boot_fill_rect(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    rw: usize,
    rh: usize,
    rgb: [u8; 3],
) {
    for y in y0..(y0 + rh).min(h) {
        for x in x0..(x0 + rw).min(w) {
            let o = (y * w + x) * RGBA8_BPP;
            buf[o] = rgb[0];
            buf[o + 1] = rgb[1];
            buf[o + 2] = rgb[2];
            buf[o + 3] = 255;
        }
    }
}

pub(crate) fn boot_bg_image_rgba_clone() -> Option<(usize, usize, Vec<u8>)> {
    boot_bg_image().map(|img| (img.width, img.height, img.rgba.clone()))
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
                let modified = meta.modified().or_else(|_| meta.created()).unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().map_or(true, |(t, _)| modified > *t) {
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
    for var in ["STEAM_COMPAT_CLIENT_INSTALL_PATH", "STEAM_HOME", "STEAM_ROOT"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                boot_bg_push_unique_root(&mut roots, std::path::PathBuf::from(value).join("userdata"));
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            let home = std::path::PathBuf::from(home);
            boot_bg_push_unique_root(&mut roots, home.join(".steam").join("steam").join("userdata"));
            boot_bg_push_unique_root(
                &mut roots,
                home.join(".local").join("share").join("Steam").join("userdata"),
            );
        }
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
            .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png"))
            .unwrap_or(false)
}

unsafe fn boot_bg_decode_wic_rgba(path: &std::path::Path) -> Option<BootBgImage> {
    // COM may already be initialized on this thread; ignore the HRESULT and let CoCreateInstance be
    // the real gate. WIC is local file decode only -- no network and no helper process.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let factory: IWICImagingFactory = unsafe {
        CoCreateInstance(&CLSID_WICImagingFactory, None::<&IUnknown>, CLSCTX_INPROC_SERVER).ok()?
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
    let len = width_usize.checked_mul(height_usize)?.checked_mul(RGBA8_BPP)?;
    let mut rgba = vec![0u8; len];
    unsafe { converted.CopyPixels(std::ptr::null(), width * RGBA8_BPP as u32, &mut rgba).ok()? };
    boot_bg_image_from_rgba(width_usize, height_usize, rgba)
}

fn boot_bg_wide_null(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
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
) {
    // Soft vignette behind the progress UI: strongest at the bar center, fading to no darkening at
    // the edges. This keeps the hairline readable over bright screenshots without a hard rectangular
    // panel or a full-screen blur pass on the launch path.
    let x0 = content_x.saturating_sub(32);
    let y0 = content_y.saturating_sub(10);
    let rw = (content_w + 64).min(w.saturating_sub(x0));
    let rh = (BOOT_VIEW_STRIP_HEIGHT + 20).min(h.saturating_sub(y0));
    if rw == 0 || rh == 0 {
        return;
    }
    let cx2 = (content_x * 2).saturating_add(content_w);
    let cy2 = (content_y * 2).saturating_add(BOOT_VIEW_STRIP_HEIGHT);
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

fn boot_draw_marker_label(
    buf: &mut [u8],
    w: usize,
    h: usize,
    content_x: usize,
    content_y: usize,
    content_w: usize,
    permille: usize,
    label: &str,
    has_bg: bool,
) -> usize {
    let marker_x = content_x + (content_w * permille / 1000).min(content_w.saturating_sub(1));
    let label_w = boot_text_width(label);
    let label_x = marker_x
        .saturating_sub(label_w / 2)
        .min(w.saturating_sub(label_w));
    if has_bg {
        boot_draw_text_shadowed(buf, w, h, label_x, content_y, label);
    } else {
        boot_draw_text(buf, w, h, label_x, content_y, label);
    }
    marker_x
}

/// Rasterize either the original tight black progress strip, or a full-screen cached screenshot
/// background with the same understated bar/label geometry overlaid near the bottom.
fn boot_view_rasterize(
    w: usize,
    h: usize,
    idx: usize,
    permille: usize,
    content_x: usize,
    content_y: usize,
    content_w: usize,
    bg: Option<&BootBgImage>,
) -> Vec<u8> {
    let mut buf = vec![0u8; w * h * RGBA8_BPP];
    let has_bg = bg.is_some();
    if let Some(bg) = bg {
        boot_fill_aspect_cover_background(&mut buf, w, h, bg);
    } else {
        boot_fill_rect(&mut buf, w, h, 0, 0, w, h, BOOT_VIEW_RGB_BLACK);
    }
    let label = BOOT_VIEW_MILESTONE_LABELS[idx.min(BOOT_VIEW_MILESTONE_LABELS.len() - 1)];
    let bar_y = content_y + BOOT_VIEW_GLYPH_H * BOOT_VIEW_TEXT_SCALE + BOOT_VIEW_TEXT_BAR_GAP;
    if has_bg {
        // Local shadow band only around the UI, plus globally dimmed screenshot: keeps the hairline bar
        // readable on bright screenshots without turning the boot screen back into a heavy panel.
        boot_darken_bar_shadow(&mut buf, w, h, content_x, content_y, content_w);
    }
    if has_bg {
        boot_draw_text_shadowed(&mut buf, w, h, content_x, content_y, label);
    } else {
        boot_draw_text(&mut buf, w, h, content_x, content_y, label);
    }
    // SAVE CHECK (the picker-hold pause, ~577) and SAVE LOAD (615) sit close together, so only one
    // is shown at a time to keep their labels from overlapping: SAVE CHECK while the missing-save
    // picker holds the boot (that is where the fill visibly pauses), SAVE LOAD otherwise (the normal
    // continue/save-load checkpoint on every boot). `usize::MAX` = not shown (tick skipped below).
    let save_picker_hold = missing_save_selection_pending();
    let save_check_marker_x = if save_picker_hold {
        boot_draw_marker_label(
            &mut buf,
            w,
            h,
            content_x,
            content_y,
            content_w,
            BOOT_VIEW_SAVE_CHECK_PERMILLE,
            BOOT_VIEW_SAVE_CHECK_LABEL,
            has_bg,
        )
    } else {
        usize::MAX
    };
    let save_load_marker_x = if save_picker_hold {
        usize::MAX
    } else {
        boot_draw_marker_label(
            &mut buf,
            w,
            h,
            content_x,
            content_y,
            content_w,
            BOOT_VIEW_SAVE_LOAD_PERMILLE,
            BOOT_VIEW_SAVE_LOAD_LABEL,
            has_bg,
        )
    };
    let marker_x = boot_draw_marker_label(
        &mut buf,
        w,
        h,
        content_x,
        content_y,
        content_w,
        BOOT_VIEW_NATIVE_HANDOFF_PERMILLE,
        BOOT_VIEW_NATIVE_HANDOFF_LABEL,
        has_bg,
    );
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
    // Save-check tick: the fill edge where the boot pauses while the overlay picker is up.
    if save_check_marker_x != usize::MAX {
        boot_fill_rect(
            &mut buf,
            w,
            h,
            save_check_marker_x.saturating_sub(BOOT_VIEW_HANDOFF_MARKER_W / 2),
            bar_y.saturating_sub(2),
            BOOT_VIEW_HANDOFF_MARKER_W,
            BOOT_VIEW_BAR_H + 4,
            BOOT_VIEW_RGB_FILL,
        );
    }
    // Save-load tick: the ShowProgressJob save-data continue/load checkpoint (shown once a save is
    // resolved, i.e. off the picker hold).
    if save_load_marker_x != usize::MAX {
        boot_fill_rect(
            &mut buf,
            w,
            h,
            save_load_marker_x.saturating_sub(BOOT_VIEW_HANDOFF_MARKER_W / 2),
            bar_y.saturating_sub(2),
            BOOT_VIEW_HANDOFF_MARKER_W,
            BOOT_VIEW_BAR_H + 4,
            BOOT_VIEW_RGB_FILL,
        );
    }
    // Handoff marker/gap: this pre-loading bar is only phase 1. Leave the remaining track empty for the
    // native now-loading Gauge_3 phase; the native loading-bar hook supplies the true terminal-frame
    // semaphore for 100%. Make the split visible: a small black break in the track, a brighter 3px marker,
    // and the NATIVE label centered above it.
    let gap_x = marker_x.saturating_sub(BOOT_VIEW_HANDOFF_GAP_W / 2);
    boot_fill_rect(
        &mut buf,
        w,
        h,
        gap_x,
        bar_y.saturating_sub(2),
        BOOT_VIEW_HANDOFF_GAP_W,
        BOOT_VIEW_BAR_H + 4,
        BOOT_VIEW_RGB_BLACK,
    );
    boot_fill_rect(
        &mut buf,
        w,
        h,
        marker_x.saturating_sub(BOOT_VIEW_HANDOFF_MARKER_W / 2),
        bar_y.saturating_sub(2),
        BOOT_VIEW_HANDOFF_MARKER_W,
        BOOT_VIEW_BAR_H + 4,
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

/// Composite the boot-progress strip onto the swapchain backbuffer. Called from the Present detour
/// for every pre-loading-window frame (the portrait composite declined). `catch_unwind` + every COM
/// call checked -> never panics on the game's render thread; any failure skips the frame.
pub(crate) unsafe fn composite_boot_progress_on_swapchain(
    _base: usize,
    swapchain_raw: usize,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        composite_boot_progress_inner(swapchain_raw, false)
    }))
    .unwrap_or(false)
}

/// Self-present-pump frame (pre-first-game-present): same draw, but the engine has NEVER rendered
/// this backbuffer, so its contents are undefined -- clear the whole RT to black before the strip
/// copy so no init-garbage flashes on screen.
pub(crate) unsafe fn composite_boot_progress_self_frame(swapchain_raw: usize) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        composite_boot_progress_inner(swapchain_raw, true)
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

unsafe fn composite_boot_progress_inner(swapchain_raw: usize, clear_first: bool) -> bool {
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) != 0 {
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
    let world_handoff = !own_menu_active && IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
    if loading_handoff || world_handoff {
        // SEAMLESS CUT (user 2026-07-06): the handoff (loading table build) starts the game's
        // black gap + the loading screen's own fade-in-from-black, so stopping here would cut a
        // lit cover into black. Instead HOLD the cover fully lit and stop in one frame only once
        // the native loading screen is itself fully lit (CS::LoadingScreen update hits reach the
        // measured luminance plateau), or immediately on world takeover, or on the bail clock if
        // the update semaphore regressed. Lit-to-lit; never a black frame between the scenes.
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
                    BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE
                        .store(LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst), Ordering::SeqCst);
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
        let hold_bail = held_ms >= BOOT_VIEW_HANDOFF_HOLD_BAIL_MS;
        if native_lit || world_handoff || hold_bail {
            if BOOT_VIEW_STOPPED.swap(1, Ordering::SeqCst) == 0 {
                BOOT_VIEW_STOP_NATIVE_HITS.store(native_hits, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "boot-view: handoff -> loading window (seamless cut; native_hits={native_hits} held_ms={held_ms} world={world_handoff} bail={hold_bail} draws={} permille={})",
                    BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
                    BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
                ));
            }
            BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.store(0, Ordering::SeqCst);
            return false;
        }
        // else: fall through and keep compositing the fully-lit cover over the native fade-in.
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
    // Only the two 8-bit RGBA/BGRA families are handled (format 28 measured on the live swapchain).
    let swap_rb = matches!(
        bb_desc.Format,
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
    );
    if !swap_rb
        && !matches!(
            bb_desc.Format,
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        )
    {
        return false;
    }

    // Progress-bar geometry follows the backbuffer. When a cached screenshot background exists, copy a
    // full-screen region; otherwise preserve the original tiny strip copy over black boot frames.
    let strip_w = (bw * BOOT_VIEW_STRIP_W_NUM / BOOT_VIEW_STRIP_W_DEN)
        .max(BOOT_VIEW_STRIP_MIN_W)
        .min(bw);
    let strip_h = (BOOT_VIEW_STRIP_HEIGHT as u32).min(bh);
    let strip_dx = (bw - strip_w) / 2;
    let strip_dy = (bh * BOOT_VIEW_STRIP_Y_NUM / BOOT_VIEW_STRIP_Y_DEN).min(bh - strip_h);
    let bg = boot_bg_image();
    let bg_active = bg.is_some();
    // The DLL-drawn startup save picker owns the whole screen while the no-save boot is held: a
    // full-frame copy of the browser, driven by the shared picker model (input handled on the game
    // task thread). Falls back to the bar if the model vanished mid-frame.
    let picker_active = save_picker_overlay_active();
    let full_frame = bg_active || picker_active;
    let (region_w, region_h, dx, dy, content_x, content_y, content_w) = if full_frame {
        (
            bw,
            bh,
            0,
            0,
            strip_dx as usize,
            strip_dy as usize,
            strip_w as usize,
        )
    } else {
        (strip_w, strip_h, strip_dx, strip_dy, 0, 0, strip_w as usize)
    };

    let (ms_idx, permille) = boot_view_progress();

    // Copyable footprint for the selected region in the backbuffer's format.
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    let region_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: region_w as u64,
        Height: region_h,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: bb_desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &region_desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total_bytes),
        )
    };
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return false;
    }
    // (Re)create the persistent upload buffer when the footprint size changes (bb resize).
    let mut upload_fresh = false;
    if BOOT_VIEW_UPLOAD_SIZE.load(Ordering::SeqCst) != total_bytes {
        let up_heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let buf_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: total_bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let mut up_opt: Option<ID3D12Resource> = None;
        if unsafe {
            device.CreateCommittedResource(
                &up_heap,
                D3D12_HEAP_FLAG_NONE,
                &buf_desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut up_opt,
            )
        }
        .is_err()
        {
            return false;
        }
        let Some(up) = up_opt else {
            return false;
        };
        let old = BOOT_VIEW_UPLOAD.swap(up.into_raw() as usize, Ordering::SeqCst);
        if old != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old as *mut c_void) });
        }
        BOOT_VIEW_UPLOAD_SIZE.store(total_bytes, Ordering::SeqCst);
        upload_fresh = true;
    }
    let up_raw = BOOT_VIEW_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
    let Some(upload) = (unsafe { ID3D12Resource::from_raw_borrowed(&up_raw) }) else {
        return false;
    };

    // Re-rasterize + rewrite the upload only when the visible content changed (or a fresh buffer).
    let geom_changed = BOOT_VIEW_STRIP_W.swap(strip_w as usize, Ordering::SeqCst)
        != strip_w as usize
        || BOOT_VIEW_STRIP_H.swap(region_h as usize, Ordering::SeqCst) != region_h as usize;
    // The picker content changes with cursor/dir/page (not captured by permille/idx), so re-raster
    // every frame while it owns the screen.
    if picker_active
        || upload_fresh
        || geom_changed
        || BOOT_VIEW_DRAWN_PERMILLE.load(Ordering::SeqCst) != permille
        || BOOT_VIEW_DRAWN_IDX.load(Ordering::SeqCst) != ms_idx
        || BOOT_VIEW_DRAWN_BG_ACTIVE.load(Ordering::SeqCst) != bg_active as usize
    {
        // Base frame is always the boot loading bar (full-frame black + the bottom strip bar). When
        // the startup picker is active it composites its browser panel ON TOP, in the upper region,
        // leaving the bar visible below -- so the bar keeps showing the boot held at SAVE_CHECK while
        // the user browses. When the picker disarms (pick resolved), the bar frame remains and the
        // boot resumes past SAVE_CHECK.
        let mut tight = boot_view_rasterize(
            region_w as usize,
            region_h as usize,
            ms_idx,
            permille,
            content_x,
            content_y,
            content_w,
            bg,
        );
        if picker_active
            && overlay_save_picker_onto(&mut tight, region_w as usize, region_h as usize)
        {
            SAVE_PICKER_OVERLAY_DRAW_HITS.fetch_add(1, Ordering::SeqCst);
        }
        let row_pitch = footprint.Footprint.RowPitch as usize;
        let total = total_bytes as usize;
        let mut umap: *mut c_void = std::ptr::null_mut();
        if unsafe { upload.Map(0, None, Some(&mut umap)) }.is_err() || umap.is_null() {
            return false;
        }
        {
            let dst = unsafe { std::slice::from_raw_parts_mut(umap as *mut u8, total) };
            let src_row = region_w as usize * RGBA8_BPP;
            for y in 0..region_h as usize {
                let so = y * src_row;
                let dofs = y * row_pitch;
                if dofs + src_row > total || so + src_row > tight.len() {
                    break;
                }
                let drow = &mut dst[dofs..dofs + src_row];
                drow.copy_from_slice(&tight[so..so + src_row]);
                if swap_rb {
                    for t in 0..region_w as usize {
                        drow.swap(t * RGBA8_BPP, t * RGBA8_BPP + 2);
                    }
                }
            }
        }
        unsafe { upload.Unmap(0, None) };
        BOOT_VIEW_DRAWN_PERMILLE.store(permille, Ordering::SeqCst);
        BOOT_VIEW_DRAWN_IDX.store(ms_idx, Ordering::SeqCst);
        BOOT_VIEW_DRAWN_BG_ACTIVE.store(bg_active as usize, Ordering::SeqCst);
    }

    // Single submit on our OWN queue: PRESENT -> COPY_DEST, strip copy, COPY_DEST -> PRESENT.
    let alloc_raw = BOOT_VIEW_ALLOCATOR.load(Ordering::SeqCst) as *mut c_void;
    let list_raw = BOOT_VIEW_LIST.load(Ordering::SeqCst) as *mut c_void;
    let fence_raw = BOOT_VIEW_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let queue_raw = BOOT_VIEW_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let (Some(allocator), Some(list), Some(fence), Some(queue)) = (unsafe {
        (
            ID3D12CommandAllocator::from_raw_borrowed(&alloc_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&list_raw),
            ID3D12Fence::from_raw_borrowed(&fence_raw),
            ID3D12CommandQueue::from_raw_borrowed(&queue_raw),
        )
    }) else {
        return false;
    };
    // Resolve the RTV heap + descriptor BEFORE opening the list: everything recorded between
    // Reset and Close below is infallible, so the list can never be left dangling open (an open
    // list would fail every subsequent Reset and silently kill the view).
    let rtv_heap_raw = BOOT_VIEW_RTV_HEAP.load(Ordering::SeqCst) as *mut c_void;
    let rtv_handle = if clear_first {
        let Some(heap) = (unsafe { ID3D12DescriptorHeap::from_raw_borrowed(&rtv_heap_raw) })
        else {
            return false;
        };
        let handle = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };
        unsafe { device.CreateRenderTargetView(&backbuffer, None, handle) };
        Some(handle)
    } else {
        None
    };
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    if let Some(handle) = rtv_handle {
        unsafe {
            record_transition(
                list,
                &backbuffer,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )
        };
        unsafe { list.ClearRenderTargetView(handle, &[0.0, 0.0, 0.0, 1.0], None) };
        unsafe {
            record_transition(
                list,
                &backbuffer,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        };
    } else {
        unsafe {
            record_transition(
                list,
                &backbuffer,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        };
    }
    let mut up_src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut bb_dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let up_box = D3D12_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: region_w,
        bottom: region_h,
        back: 1,
    };
    unsafe { list.CopyTextureRegion(&bb_dst, dx, dy, 0, &up_src, Some(&up_box)) };
    unsafe { ManuallyDrop::drop(&mut up_src.pResource) };
    unsafe { ManuallyDrop::drop(&mut bb_dst.pResource) };
    unsafe {
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PRESENT,
        )
    };
    if !unsafe { execute_and_wait(queue, list, fence) } {
        return false;
    }

    let hits = BOOT_VIEW_DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "boot-view: first draw onto backbuffer {bw}x{bh} (region {region_w}x{region_h} at {dx},{dy}, bg={}, permille={permille})",
            bg_active as usize
        ));
    }
    true
}
