// Boot-progress view -- our own pre-Continue cover content, drawn from the FIRST presented frame.
//
// With the splash/logo/title visuals suppressed, every frame the game presents between its first
// `Present` (~+3.5s after attach) and the post-Continue loading window (~+15.5s) is pure black. The
// Present-hook VMT swap is already installed BEFORE the first present (task tick ~+3.0s), so the
// black gap is a draw-gating matter, not a hook-timing one: this module opens the gate at Present
// hit #1 with content that needs NOTHING from the game -- a procedurally rasterized strip (panel +
// milestone label + progress bar, 5x7 embedded font, no game-derived assets) whose progress is
// driven purely by our own already-latched RAM semaphores:
//
//   BOOT     -- drawing at all (present hook + swapchain live)
//   GAME     -- `game_man_ptr_or_null() != 0` (GameMan constructed)
//   OFFLINE  -- `FORCE_OFFLINE_BYTES_CLEARED` (GameMan online bytes cleared, ~+8.5s)
//   TITLE    -- `TITLE_FADEIN_SKIP_FIRED` (zero-input FadeIn->Loop transition)
//   MENU     -- `PRODUCT_CORE_LAST_MENU_OPENED_LATCH` (title menu natural-open latch)
//   CONTINUE -- `SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT` / `TFC_CONTINUE_FIRED`
//   LOADING  -- `PROFILE_LOADSCREEN_TABLE_BUILDS > 0` -> HANDOFF (stop; the loading-portrait
//               overlay window owns the screen from here)
//
// Reached milestones are latched into a monotonic bitmask (a latch that later reads 0 cannot walk
// the bar backwards), and the displayed value creeps part-way toward the next milestone over time so
// the bar visibly moves between semaphores. The draw is a single submit on our OWN queue (transition
// PRESENT->COPY_DEST, CopyTextureRegion upload->backbuffer strip rect, transition back, CPU fence
// wait) -- no backbuffer readback: the pre-Continue frames are the content-free black this view
// exists to replace, and the strip rect is entirely ours.

/// Draw-state machine: 0 = uninit, 1 = ready, 2 = failed (give up; never retry).
static BOOT_VIEW_DRAW_STATE: AtomicUsize = AtomicUsize::new(0);
/// One-shot stop latch: the loading window / world took over; the boot view never draws again.
static BOOT_VIEW_STOPPED: AtomicUsize = AtomicUsize::new(0);
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
/// (w, h) the current upload buffer was rasterized for (strip geometry follows the backbuffer).
static BOOT_VIEW_STRIP_W: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_STRIP_H: AtomicUsize = AtomicUsize::new(0);
/// Last (permille, idx) actually rasterized into the upload buffer (skip the map/write when unchanged).
static BOOT_VIEW_DRAWN_PERMILLE: AtomicUsize = AtomicUsize::new(usize::MAX);
static BOOT_VIEW_DRAWN_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Creep timing epoch + the epoch-ms when the milestone index last advanced.
static BOOT_VIEW_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
static BOOT_VIEW_IDX_CHANGED_MS: AtomicU64 = AtomicU64::new(0);

/// Milestone labels (5x7 font glyph coverage: A-Z subset + digits + '%'; see `boot_glyph_5x7`).
const BOOT_VIEW_MILESTONE_LABELS: [&str; 7] = [
    "BOOT", "GAME", "OFFLINE", "TITLE", "MENU", "CONTINUE", "LOADING",
];
/// Progress targets per milestone, in permille. Spacing follows the measured product-run timeline
/// (first present +3.5s, offline +8.5s, title/menu ~+10s, continue ~+15s, table builds ~+15.5s) so
/// the bar's pace roughly matches wall-clock without ever depending on it.
const BOOT_VIEW_MILESTONE_PERMILLE: [usize; 7] = [60, 200, 350, 550, 700, 880, 1000];
/// Inter-milestone creep: over this window the bar moves up to 7/10 of the gap to the next target.
const BOOT_VIEW_CREEP_FULL_MS: u64 = 6000;
const BOOT_VIEW_CREEP_NUM: usize = 7;
const BOOT_VIEW_CREEP_DEN: usize = 10;

// Strip geometry (pixels; text is the 5x7 font at 2x = 10x14).
const BOOT_VIEW_BORDER: usize = 1;
const BOOT_VIEW_PAD: usize = 6;
const BOOT_VIEW_TEXT_SCALE: usize = 2;
const BOOT_VIEW_GLYPH_W: usize = 5;
const BOOT_VIEW_GLYPH_H: usize = 7;
/// Advance per character (5px glyph + 1px gap, pre-scale).
const BOOT_VIEW_GLYPH_ADV: usize = 6;
const BOOT_VIEW_BAR_H: usize = 8;
/// Gap between the text row and the bar track.
const BOOT_VIEW_TEXT_BAR_GAP: usize = 4;
/// Total strip height: border+pad, text row, gap, bar, pad+border.
const BOOT_VIEW_STRIP_HEIGHT: usize = 2 * (BOOT_VIEW_BORDER + BOOT_VIEW_PAD)
    + BOOT_VIEW_GLYPH_H * BOOT_VIEW_TEXT_SCALE
    + BOOT_VIEW_TEXT_BAR_GAP
    + BOOT_VIEW_BAR_H;
/// Strip width = backbuffer width * NUM/DEN (clamped to a sane minimum).
const BOOT_VIEW_STRIP_W_NUM: u32 = 11;
const BOOT_VIEW_STRIP_W_DEN: u32 = 20;
const BOOT_VIEW_STRIP_MIN_W: u32 = 220;
/// Strip top edge = backbuffer height * NUM/DEN (lower third, where the game's own bar lives).
const BOOT_VIEW_STRIP_Y_NUM: u32 = 78;
const BOOT_VIEW_STRIP_Y_DEN: u32 = 100;

// Palette (R, G, B) -- muted panel + the gold accent family of the game's own UI.
const BOOT_VIEW_RGB_PANEL: [u8; 3] = [16, 15, 13];
const BOOT_VIEW_RGB_BORDER: [u8; 3] = [96, 82, 46];
const BOOT_VIEW_RGB_TRACK: [u8; 3] = [30, 28, 24];
const BOOT_VIEW_RGB_FILL: [u8; 3] = [198, 161, 74];
const BOOT_VIEW_RGB_TEXT: [u8; 3] = [214, 198, 152];

/// True once milestone `idx`'s semaphore has asserted. Every predicate is a pure atomic/pointer read
/// that is safe from the render thread; ordering mistakes degrade to a stalled bar, never a lie about
/// sequence (the reached MASK is latched monotonic by the caller).
fn boot_milestone_reached(idx: usize) -> bool {
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
        6 => PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst) != 0,
        _ => false,
    }
}

/// Compute the current (milestone idx, displayed permille). Latches newly reached milestones into the
/// monotonic mask, stamps idx-change time for the creep, and never lets the displayed value decrease.
fn boot_view_progress() -> (usize, usize) {
    let mut mask = BOOT_VIEW_REACHED_MASK.load(Ordering::SeqCst);
    for i in 0..BOOT_VIEW_MILESTONE_LABELS.len() {
        if mask & (1 << i) == 0 && boot_milestone_reached(i) {
            mask |= 1 << i;
        }
    }
    BOOT_VIEW_REACHED_MASK.store(mask, Ordering::SeqCst);
    let idx = (usize::BITS - 1 - mask.max(1).leading_zeros()) as usize;
    let epoch = *BOOT_VIEW_EPOCH.get_or_init(std::time::Instant::now);
    let now_ms = epoch.elapsed().as_millis().min(u64::MAX as u128) as u64;
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
        1000
    };
    let since = now_ms.saturating_sub(BOOT_VIEW_IDX_CHANGED_MS.load(Ordering::SeqCst));
    let creep = (next.saturating_sub(base) * (since.min(BOOT_VIEW_CREEP_FULL_MS) as usize)
        * BOOT_VIEW_CREEP_NUM)
        / (BOOT_VIEW_CREEP_FULL_MS as usize * BOOT_VIEW_CREEP_DEN);
    let pm = (base + creep).min(1000);
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
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
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

/// Blit `text` into the tight RGBA buffer at (x, y), scaled by `BOOT_VIEW_TEXT_SCALE`.
fn boot_draw_text(buf: &mut [u8], w: usize, h: usize, x: usize, y: usize, text: &str) {
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
                            buf[o] = BOOT_VIEW_RGB_TEXT[0];
                            buf[o + 1] = BOOT_VIEW_RGB_TEXT[1];
                            buf[o + 2] = BOOT_VIEW_RGB_TEXT[2];
                            buf[o + 3] = 255;
                        }
                    }
                }
            }
        }
        cx += BOOT_VIEW_GLYPH_ADV * BOOT_VIEW_TEXT_SCALE;
    }
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

/// Rasterize the full strip: panel + border, milestone label (left), percent (right), bar track with
/// milestone tick marks and the gold fill up to `permille`.
fn boot_view_rasterize(w: usize, h: usize, idx: usize, permille: usize) -> Vec<u8> {
    let mut buf = vec![0u8; w * h * RGBA8_BPP];
    boot_fill_rect(&mut buf, w, h, 0, 0, w, h, BOOT_VIEW_RGB_PANEL);
    // 1px border.
    boot_fill_rect(&mut buf, w, h, 0, 0, w, BOOT_VIEW_BORDER, BOOT_VIEW_RGB_BORDER);
    boot_fill_rect(
        &mut buf,
        w,
        h,
        0,
        h - BOOT_VIEW_BORDER,
        w,
        BOOT_VIEW_BORDER,
        BOOT_VIEW_RGB_BORDER,
    );
    boot_fill_rect(&mut buf, w, h, 0, 0, BOOT_VIEW_BORDER, h, BOOT_VIEW_RGB_BORDER);
    boot_fill_rect(
        &mut buf,
        w,
        h,
        w - BOOT_VIEW_BORDER,
        0,
        BOOT_VIEW_BORDER,
        h,
        BOOT_VIEW_RGB_BORDER,
    );
    let inset = BOOT_VIEW_BORDER + BOOT_VIEW_PAD;
    // Label (left) + percent (right) on the text row.
    let label = BOOT_VIEW_MILESTONE_LABELS[idx.min(BOOT_VIEW_MILESTONE_LABELS.len() - 1)];
    boot_draw_text(&mut buf, w, h, inset, inset, label);
    let pct = format!("{}%", permille / 10);
    let pct_w = pct.chars().count() * BOOT_VIEW_GLYPH_ADV * BOOT_VIEW_TEXT_SCALE;
    boot_draw_text(&mut buf, w, h, w.saturating_sub(inset + pct_w), inset, &pct);
    // Bar track + fill + milestone ticks.
    let bar_y = inset + BOOT_VIEW_GLYPH_H * BOOT_VIEW_TEXT_SCALE + BOOT_VIEW_TEXT_BAR_GAP;
    let track_x = inset;
    let track_w = w.saturating_sub(2 * inset);
    boot_fill_rect(
        &mut buf,
        w,
        h,
        track_x,
        bar_y,
        track_w,
        BOOT_VIEW_BAR_H,
        BOOT_VIEW_RGB_TRACK,
    );
    boot_fill_rect(
        &mut buf,
        w,
        h,
        track_x,
        bar_y,
        track_w * permille.min(1000) / 1000,
        BOOT_VIEW_BAR_H,
        BOOT_VIEW_RGB_FILL,
    );
    for &p in &BOOT_VIEW_MILESTONE_PERMILLE {
        if p == 0 || p >= 1000 {
            continue;
        }
        boot_fill_rect(
            &mut buf,
            w,
            h,
            track_x + track_w * p / 1000,
            bar_y,
            1,
            BOOT_VIEW_BAR_H,
            BOOT_VIEW_RGB_BORDER,
        );
    }
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
        composite_boot_progress_inner(swapchain_raw)
    }))
    .unwrap_or(false)
}

unsafe fn composite_boot_progress_inner(swapchain_raw: usize) -> bool {
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) != 0 {
        return false;
    }
    // HANDOFF: the post-Continue loading window (table builds / a published keyed head) or the world
    // itself owns the screen now. Permanent stop -- the boot view exists only for the pre-Continue
    // black gap. NOTE: `now_loading_active` is deliberately NOT consulted: its `load_done` latch is
    // false during boot too, so it cannot distinguish "booting" from "loading".
    if PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst) != 0
        || PROFILE_HAVE_KEYED_FRAME.load(Ordering::SeqCst) != 0
        || IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES
    {
        if BOOT_VIEW_STOPPED.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "boot-view: handoff -> loading window (draws={} permille={} mask=0x{:x})",
                BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
                BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
                BOOT_VIEW_REACHED_MASK.load(Ordering::SeqCst),
            ));
        }
        return false;
    }
    if BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst) == 2 {
        return false;
    }

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

    // Strip geometry follows the backbuffer.
    let strip_w = (bw * BOOT_VIEW_STRIP_W_NUM / BOOT_VIEW_STRIP_W_DEN)
        .max(BOOT_VIEW_STRIP_MIN_W)
        .min(bw);
    let strip_h = (BOOT_VIEW_STRIP_HEIGHT as u32).min(bh);
    let dx = (bw - strip_w) / 2;
    let dy = (bh * BOOT_VIEW_STRIP_Y_NUM / BOOT_VIEW_STRIP_Y_DEN).min(bh - strip_h);

    let (ms_idx, permille) = boot_view_progress();

    // Copyable footprint for a strip_w x strip_h region in the backbuffer's format.
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
        Width: strip_w as u64,
        Height: strip_h,
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
        || BOOT_VIEW_STRIP_H.swap(strip_h as usize, Ordering::SeqCst) != strip_h as usize;
    if upload_fresh
        || geom_changed
        || BOOT_VIEW_DRAWN_PERMILLE.load(Ordering::SeqCst) != permille
        || BOOT_VIEW_DRAWN_IDX.load(Ordering::SeqCst) != ms_idx
    {
        let tight = boot_view_rasterize(strip_w as usize, strip_h as usize, ms_idx, permille);
        let row_pitch = footprint.Footprint.RowPitch as usize;
        let total = total_bytes as usize;
        let mut umap: *mut c_void = std::ptr::null_mut();
        if unsafe { upload.Map(0, None, Some(&mut umap)) }.is_err() || umap.is_null() {
            return false;
        }
        {
            let dst = unsafe { std::slice::from_raw_parts_mut(umap as *mut u8, total) };
            let src_row = strip_w as usize * RGBA8_BPP;
            for y in 0..strip_h as usize {
                let so = y * src_row;
                let dofs = y * row_pitch;
                if dofs + src_row > total || so + src_row > tight.len() {
                    break;
                }
                let drow = &mut dst[dofs..dofs + src_row];
                drow.copy_from_slice(&tight[so..so + src_row]);
                if swap_rb {
                    for t in 0..strip_w as usize {
                        drow.swap(t * RGBA8_BPP, t * RGBA8_BPP + 2);
                    }
                }
            }
        }
        unsafe { upload.Unmap(0, None) };
        BOOT_VIEW_DRAWN_PERMILLE.store(permille, Ordering::SeqCst);
        BOOT_VIEW_DRAWN_IDX.store(ms_idx, Ordering::SeqCst);
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
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    unsafe {
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
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
        right: strip_w,
        bottom: strip_h,
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
            "boot-view: first draw onto backbuffer {bw}x{bh} (strip {strip_w}x{strip_h} at {dx},{dy}, permille={permille})"
        ));
    }
    true
}
