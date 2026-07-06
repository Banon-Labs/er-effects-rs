// === Loading-screen player-stats text (er-effects-rs-jsm) =========================================
//
// PIVOT (user 2026-07-06): rather than fight to layer the head UNDER the native tips, we CONTROL the
// surface -- suppress the native loading tips + "press to advance" key guide, and render OUR OWN text
// (the local character's stats) on top of the head in the Present overlay, using the GAME'S OWN menu
// font via `er_gfx::raster::RasterFont`. This section is the font-independent text-layout renderer; the
// font source, the stats read, and the overlay composite are wired alongside it.

/// Render a stack of left-aligned text `lines` to a tightly-packed RGBA8 bitmap using the parsed game
/// menu font `font`, at `em_px` glyph height. Each glyph's coverage becomes `color` (RGB = color.rgb,
/// alpha = coverage * color.a / 255), so the result composites straight over the head. Returns
/// `(width, height, rgba)` sized to the glyphs' bounding box plus a 1px pad, or `(0,0,vec![])` if nothing
/// rendered. Pure CPU, no game state; safe to call from any thread.
pub(crate) fn render_lines_to_rgba(
    font: &er_gfx::raster::RasterFont,
    lines: &[String],
    em_px: f32,
    color: [u8; 4],
) -> (u32, u32, Vec<u8>) {
    if em_px < 1.0 || lines.is_empty() {
        return (0, 0, Vec::new());
    }
    let scale = font.scale_for_em_px(em_px);
    let line_h = font.line_height_px(scale).max(em_px);
    let ascent = font.ascent_px(scale).max(em_px * 0.8);
    // Placement pass: collect each glyph bitmap with its top-left destination position (in an unclamped
    // coordinate space whose origin is the first line's pen origin), and track the bounding box.
    struct Placed {
        bmp: er_gfx::raster::GlyphBitmap,
        x: f32,
        y: f32,
    }
    let mut placed: Vec<Placed> = Vec::new();
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for (li, line) in lines.iter().enumerate() {
        let baseline = ascent + li as f32 * line_h;
        let mut pen_x = 0.0f32;
        for ch in line.chars() {
            let adv = font.advance_px(ch, scale);
            if let Some(bmp) = font.rasterize(ch, scale) {
                let gx = pen_x + bmp.left as f32;
                let gy = baseline + bmp.top as f32;
                min_x = min_x.min(gx);
                min_y = min_y.min(gy);
                max_x = max_x.max(gx + bmp.width as f32);
                max_y = max_y.max(gy + bmp.height as f32);
                placed.push(Placed {
                    bmp,
                    x: gx,
                    y: gy,
                });
            }
            pen_x += adv;
        }
        // Ensure an empty/space-only line still advances the bounding box vertically.
        min_y = min_y.min(baseline - ascent);
        max_y = max_y.max(baseline - ascent + line_h);
        min_x = min_x.min(0.0);
    }
    if placed.is_empty() || max_x <= min_x || max_y <= min_y {
        return (0, 0, Vec::new());
    }
    // Drop shadow: same formula as the custom loading bar (`boot_draw_text_shadowed`) -- an opaque black
    // copy offset by (+SHADOW, +SHADOW) rendered UNDER the text. Pad the bitmap by the shadow offset.
    const SHADOW: i32 = 2;
    let pad = 1.0f32;
    let w = (max_x - min_x + 2.0 * pad).ceil() as u32 + SHADOW as u32;
    let h = (max_y - min_y + 2.0 * pad).ceil() as u32 + SHADOW as u32;
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return (0, 0, Vec::new());
    }
    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    // Alpha-OVER composite one glyph's coverage as `col`, at destination origin `(dx0, dy0)`.
    let blit = |rgba: &mut [u8], p: &Placed, dx0: i32, dy0: i32, col: [u8; 4]| {
        let (ca, cr, cg, cb) = (col[3] as u32, col[0] as u32, col[1] as u32, col[2] as u32);
        for sy in 0..p.bmp.height as i32 {
            let dy = dy0 + sy;
            if dy < 0 || dy >= h as i32 {
                continue;
            }
            for sx in 0..p.bmp.width as i32 {
                let dx = dx0 + sx;
                if dx < 0 || dx >= w as i32 {
                    continue;
                }
                let cov =
                    p.bmp.coverage[(sy as usize) * (p.bmp.width as usize) + sx as usize] as u32;
                if cov == 0 {
                    continue;
                }
                let a = cov * ca / 255;
                if a == 0 {
                    continue;
                }
                let o = ((dy as usize) * (w as usize) + dx as usize) * 4;
                let ia = 255 - a;
                rgba[o] = ((cr * a + rgba[o] as u32 * ia) / 255) as u8;
                rgba[o + 1] = ((cg * a + rgba[o + 1] as u32 * ia) / 255) as u8;
                rgba[o + 2] = ((cb * a + rgba[o + 2] as u32 * ia) / 255) as u8;
                rgba[o + 3] = (a + rgba[o + 3] as u32 * ia / 255).min(255) as u8;
            }
        }
    };
    // Pass 1: black shadow (offset). Pass 2: the coloured text (over the shadow).
    for p in &placed {
        let dx0 = (p.x - min_x + pad).round() as i32;
        let dy0 = (p.y - min_y + pad).round() as i32;
        blit(&mut rgba, p, dx0 + SHADOW, dy0 + SHADOW, [0, 0, 0, color[3]]);
    }
    for p in &placed {
        let dx0 = (p.x - min_x + pad).round() as i32;
        let dy0 = (p.y - min_y + pad).round() as i32;
        blit(&mut rgba, p, dx0, dy0, color);
    }
    (w, h, rgba)
}

// --- Game menu font: captured at runtime from the game's own Scaleform file-open, or from an env
// --- diagnostic .gfx on disk. NOTHING is embedded (per the no-game-derived-binaries rule).
/// Raw captured `font.gfx` bytes (copied out of the game's Scaleform MemoryFile in the file-open hook).
static MENU_FONT_GFX_CAPTURED: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
/// The parsed, cached menu font (built once from the captured `font.gfx` bytes).
static MENU_FONT_RASTER: std::sync::OnceLock<er_gfx::raster::RasterFont> = std::sync::OnceLock::new();

/// Capture the game's menu font from the Scaleform file-open hook. Reads the returned MemoryFile's raw
/// GFX payload (same guarded read `title_05_000_swap_to_stripped` uses) and stores a COPY (never retains
/// the game pointer). Called for any file-open whose URL looks like the menu font; one-shot.
pub(crate) unsafe fn capture_menu_font_gfx(base: usize, file: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if MENU_FONT_GFX_CAPTURED.get().is_some() || base == 0 || base == null || file == 0 || file == null
    {
        return;
    }
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return;
    }
    let data = unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
    let len = unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
    if data == 0 || len < 8 || len > 64 * 1024 * 1024 {
        return;
    }
    // Probe magic + both ends through the guarded reader before touching the range.
    let magic = unsafe { safe_read_u8(data) }.unwrap_or(0);
    let last = unsafe { safe_read_u8(data + len as usize - 1) };
    if magic != b'G' || last.is_none() {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(data as *const u8, len as usize) }.to_vec();
    if !bytes.starts_with(b"GFX") {
        return;
    }
    let n = bytes.len();
    let _ = MENU_FONT_GFX_CAPTURED.set(bytes);
    append_autoload_debug(format_args!(
        "stats-text: captured menu font gfx from file-open ({n} bytes); will parse a DefineFont3"
    ));
}

/// Parse `.gfx` bytes and build a `RasterFont` from the DefineFont3 with the most glyphs (best ASCII
/// coverage), recursing into DefineSprite. `None` if no font tag decodes.
fn build_menu_font_from_gfx(bytes: &[u8]) -> Option<er_gfx::raster::RasterFont> {
    let movie = er_gfx::Movie::parse(bytes).ok()?;
    fn best_font<'a>(tags: &'a [er_gfx::Tag], best: &mut Option<&'a er_gfx::Tag>, best_n: &mut usize) {
        for t in tags {
            if let er_gfx::Tag::DefineFont3 { glyphs, codes, .. } = t {
                if glyphs.len() == codes.len() && glyphs.len() > *best_n {
                    *best_n = glyphs.len();
                    *best = Some(t);
                }
            }
            if let er_gfx::Tag::DefineSprite { tags, .. } = t {
                best_font(tags, best, best_n);
            }
        }
    }
    let mut best = None;
    let mut best_n = 0;
    best_font(&movie.tags, &mut best, &mut best_n);
    er_gfx::raster::RasterFont::from_define_font3(best?)
}

/// The game's menu font, parsed + cached from the runtime file-open capture of `font.gfx`. `None` until
/// the capture has happened and the font parses (product path only -- no env crutch).
pub(crate) fn menu_font() -> Option<&'static er_gfx::raster::RasterFont> {
    if let Some(f) = MENU_FONT_RASTER.get() {
        return Some(f);
    }
    let bytes = MENU_FONT_GFX_CAPTURED.get()?;
    let font = build_menu_font_from_gfx(bytes)?;
    let _ = MENU_FONT_RASTER.set(font);
    append_autoload_debug(format_args!(
        "stats-text: menu font parsed from CAPTURED font.gfx ({} glyphs)",
        MENU_FONT_RASTER.get().map(|f| f.glyph_count()).unwrap_or(0)
    ));
    MENU_FONT_RASTER.get()
}

/// The local character's loading-screen stats (er-effects-rs-jsm). Read from the loading-screen-safe
/// ProfileSummary record (name/level/playtime) + live PlayerGameData when up (attributes, HP/FP/Stamina),
/// falling back to the `.sl2` for attributes pre-load.
pub(crate) struct LoadingScreenStats {
    pub name: String,
    pub level: i32,
    pub attributes: [i32; 8], // VIG,MND,END,STR,DEX,INT,FAI,ARC
    pub max_hp: u32,
    pub max_fp: u32,
    pub max_stamina: u32,
    pub play_time_ms: u32,
    pub attr_source_live: bool,
}

/// Read the local character's stats for the loading screen. `None` if GameDataMan is not up. Prefers
/// sources valid pre-load; guards every read.
pub(crate) unsafe fn read_loading_screen_stats() -> Option<LoadingScreenStats> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let gdm = game_data_man_ptr_or_null();
    if !valid(gdm) {
        return None;
    }
    // Slot source = the make-before-break portrait target (second-character fix, user-reported
    // 2026-07-06): during a System-Quit switch the user-picked slot (SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT,
    // set at the confirm press) names the character being LOADED, while ac0 still names the resident OLD
    // character until the deserialize flips it -- so the ac0-first read rendered character 1's record
    // (which the still-resident char-1 PGD then "validated" as live) under character 2's loading screen.
    // Same priority as portrait_target_slot(), keeping the boot-time best_active_slot fallback.
    let sel = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    let slot = if sel <= i32::MAX as usize
        && (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&(sel as i32))
    {
        sel as i32
    } else {
        portrait_loaded_slot_confirmed().unwrap_or_else(|| unsafe { best_active_slot() })
    };
    let slot_u = if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        slot as usize
    } else {
        return None;
    };
    // ProfileSummary record: name / level / playtime (populated before the load).
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(0);
    let mut name = String::new();
    let mut level = 0i32;
    let mut record_play_ms = 0u32;
    if valid(summary) {
        let rec = summary + PROFILE_SUMMARY_RECORD_BASE + slot_u * PROFILE_SUMMARY_RECORD_STRIDE;
        let (units, len) = unsafe { read_utf16_name_units(rec) };
        name = String::from_utf16_lossy(&units[..len]);
        level = unsafe { safe_read_i32(rec + PROFILE_SUMMARY_LEVEL_OFFSET) }.unwrap_or(0);
        // The summary record stores playtime in SECONDS (runtime-observed 2026-07-06: record 390000
        // == 108:20:00 vs live GDM 108:22:13 ms counter), so scale to the struct's ms unit.
        record_play_ms = (unsafe { safe_read_i32(rec + PROFILE_SUMMARY_PLAYTIME_OFFSET) }
            .unwrap_or(0) as u32)
            .saturating_mul(1000);
    }
    // Live PlayerGameData ONLY if it provably holds the LOADING slot's character. Before the save
    // deserializes, PGD is the game's default level-9 template (name empty, stats
    // [15,10,11,14,13,9,9,7]) -- NOT the slot being loaded -- so trusting it renders another
    // character's stats under the right name (user-reported 2026-07-06). Prove ownership by matching
    // the slot-scoped ProfileSummary record: identical non-empty name AND identical level.
    let pgd = unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }
        .filter(|&p| valid(p));
    let pgd_validated = pgd.filter(|&pgd| {
        let (ln, ll) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
        let pgd_level = unsafe { safe_read_i32(pgd + PGD_LEVEL_68_OFFSET) }.unwrap_or(0);
        ll > 0
            && pgd_level > 0
            && pgd_level == level
            && String::from_utf16_lossy(&ln[..ll]) == name
    });
    let (attributes, max_hp, max_fp, max_stamina, attr_source_live) =
        if let Some(pgd) = pgd_validated {
            let mut a = [0i32; 8];
            for (i, v) in a.iter_mut().enumerate() {
                *v = unsafe { safe_read_i32(pgd + PGD_STAT_BASE_3C_OFFSET + i * 4) }.unwrap_or(0);
            }
            (
                a,
                unsafe { safe_read_i32(pgd + PGD_CURRENT_MAX_HP_14_OFFSET) }.unwrap_or(0) as u32,
                unsafe { safe_read_i32(pgd + PGD_CURRENT_MAX_FP_20_OFFSET) }.unwrap_or(0) as u32,
                unsafe { safe_read_i32(pgd + PGD_CURRENT_MAX_STAMINA_30_OFFSET) }.unwrap_or(0)
                    as u32,
                true,
            )
        } else {
            let base = game_module_base().unwrap_or(null);
            if valid(base) {
                let _ = ensure_profile_slot_stats_cached(base);
            }
            let attrs = profile_slot_attributes(slot).unwrap_or([0; 8]);
            (attrs, 0, 0, 0, false)
        };
    // Playtime is slot-scoped from the record; the global GDM counter only reflects the loaded
    // character after deserialize, so it is trusted only alongside a validated PGD.
    let play_time_ms = if attr_source_live {
        unsafe { safe_read_i32(gdm + GDM_PLAY_TIME_A0_OFFSET) }
            .map(|v| v as u32)
            .filter(|&v| v != 0)
            .unwrap_or(record_play_ms)
    } else {
        record_play_ms
    };
    Some(LoadingScreenStats {
        name,
        level,
        attributes,
        max_hp,
        max_fp,
        max_stamina,
        play_time_ms,
        attr_source_live,
    })
}

/// `eldenring::cs::GameDataMan::play_time` (+0xa0, milliseconds) -- bound to the upstream struct so a
/// layout drift fails the build rather than reading garbage.
const GDM_PLAY_TIME_A0_OFFSET: usize = core::mem::offset_of!(eldenring::cs::GameDataMan, play_time);

/// Format a play-time in ms as `H:MM:SS`.
fn fmt_playtime(ms: u32) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Lay the stats out as display lines (user pick: name/level/playtime + 8 attributes + derived HP/FP/Stm).
pub(crate) fn format_stats_lines(st: &LoadingScreenStats) -> Vec<String> {
    let a = &st.attributes;
    let name = if st.name.trim().is_empty() {
        "Tarnished".to_string()
    } else {
        st.name.clone()
    };
    let mut lines = vec![
        name,
        format!("Level {}    Time  {}", st.level, fmt_playtime(st.play_time_ms)),
    ];
    if st.max_hp > 0 || st.max_fp > 0 || st.max_stamina > 0 {
        lines.push(format!(
            "HP {}    FP {}    Stamina {}",
            st.max_hp, st.max_fp, st.max_stamina
        ));
    }
    lines.push(format!(
        "VIG {}   MND {}   END {}   STR {}",
        a[0], a[1], a[2], a[3]
    ));
    lines.push(format!(
        "DEX {}   INT {}   FTH {}   ARC {}",
        a[4], a[5], a[6], a[7]
    ));
    lines
}

/// The rendered loading-screen stats text, keyed by the exact display `lines` it renders. The game
/// thread rebuilds it whenever the loading slot's lines differ (character switch, record->live upgrade,
/// playtime tick); the render thread composites it. ONE mutex guards bitmap + key together so a window
/// reset racing a build can never strand a key without its bitmap (which would suppress rebuilds and
/// blank the text for the whole window).
pub(crate) struct StatsTextCache {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// The exact lines the bitmap renders -- the rebuild key.
    pub lines: Vec<String>,
}
pub(crate) static STATS_TEXT_CACHE: std::sync::Mutex<Option<StatsTextCache>> =
    std::sync::Mutex::new(None);
/// Cumulative stats-bitmap build count (telemetry oracle `oracle_stats_text_built`; never reset).
pub(crate) static STATS_TEXT_BUILT: AtomicUsize = AtomicUsize::new(0);
/// `(name, level, live)` of the last logged build -- gates the debug log so per-second playtime rebuilds
/// don't spam it, while identity changes (new character, record->live upgrade) still log.
static STATS_TEXT_LOGGED: std::sync::Mutex<Option<(String, i32, bool)>> =
    std::sync::Mutex::new(None);

/// Build the stats-text bitmap from the slot's stats + game menu font, into `STATS_TEXT_CACHE`.
/// CONTENT-KEYED (second-character fix, user-reported 2026-07-06): rebuild exactly when the loading
/// slot's formatted lines differ from what is currently rendered, never a per-window one-shot latch.
/// The old `STATS_TEXT_LIVE` latch re-armed AFTER the window reset (this tick keeps running until
/// load_done + cover-down go idle) with the PREVIOUS character's still-resident PlayerGameData, so a
/// System-Quit switch showed character 1's stats through character 2's entire loading screen. With
/// content keying a stale bitmap self-heals the moment the new slot's record reads differently, and
/// identical ticks stay cheap no-ops. Called on the loading screen from the game thread; silently waits
/// until both the captured font and readable stats exist.
pub(crate) unsafe fn maybe_build_stats_text() {
    let Some(font) = menu_font() else {
        return;
    };
    let Some(stats) = (unsafe { read_loading_screen_stats() }) else {
        return;
    };
    // Wait for real content (a non-empty name or a real level) before rendering anything.
    if stats.level <= 0 && stats.name.trim().is_empty() {
        return;
    }
    let lines = format_stats_lines(&stats);
    let unchanged = STATS_TEXT_CACHE
        .lock()
        .ok()
        .is_some_and(|g| g.as_ref().is_some_and(|c| c.lines == lines));
    if unchanged {
        return;
    }
    let (w, h, rgba) = render_lines_to_rgba(font, &lines, 48.0, [238, 228, 202, 255]);
    if w == 0 || h == 0 {
        return;
    }
    if let Ok(mut g) = STATS_TEXT_CACHE.lock() {
        *g = Some(StatsTextCache {
            w,
            h,
            rgba,
            lines: lines.clone(),
        });
    }
    STATS_TEXT_BUILT.fetch_add(1, Ordering::SeqCst);
    let ident = (stats.name.clone(), stats.level, stats.attr_source_live);
    let fresh_ident = STATS_TEXT_LOGGED.lock().ok().is_none_or(|mut g| {
        if g.as_ref() == Some(&ident) {
            false
        } else {
            *g = Some(ident);
            true
        }
    });
    if fresh_ident {
        append_autoload_debug(format_args!(
            "stats-text: built loading-screen stats bitmap {w}x{h} (live={}) lines={:?}",
            stats.attr_source_live, lines
        ));
    }
}

/// Composite the cached stats text into the head RGBA `spx` (`sw`x`sh`) at the lower-left, at the text's
/// native pixel size. The head is depth-alpha-keyed, so the region under the text is transparent -- the
/// opaque text pixels show on screen while the surrounding transparent pixels still show the loading
/// movie; the head silhouette (upper) is untouched. Reuses the whole existing overlay draw (no new GPU
/// pipeline). Called from the overlay fill each frame (the head under the static text updates per frame).
pub(crate) fn composite_stats_into_head(spx: &mut [u8], sw: u32, sh: u32) {
    let Some((tw, th, tpx)) = STATS_TEXT_CACHE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| (c.w, c.h, c.rgba.clone())))
    else {
        return;
    };
    if tw == 0 || th == 0 || sw == 0 || sh == 0 {
        return;
    }
    // Lower-left of the head texture; if the text is wider/taller than fits, it clips (blend_rgba_over
    // clamps). Positions chosen so the aspect-cover to the 16:9 backbuffer lands it in the lower third.
    let x0 = (sw as f32 * 0.05) as i32;
    let y0 = (sh as f32 * 0.60) as i32;
    blend_rgba_over(spx, sw, sh, &tpx, tw, th, x0, y0);
}

/// Reset the per-load stats-text cache so the next load starts from a clean (no-text) frame and its
/// first build logs. Correctness does NOT depend on this reset: the content key in
/// `maybe_build_stats_text` rebuilds on any line change even if a post-reset tick re-caches the old
/// character. `STATS_TEXT_BUILT` is a cumulative oracle and is deliberately not reset.
pub(crate) fn stats_text_window_reset() {
    if let Ok(mut g) = STATS_TEXT_CACHE.lock() {
        *g = None;
    }
    if let Ok(mut g) = STATS_TEXT_LOGGED.lock() {
        *g = None;
    }
}

/// Tip-refresh detour: NO-OP the original (er-effects-rs-jsm PIVOT) so the native tip title/body are never
/// set and the `Main` tip clip stays faded out -- our overlay player-stats text owns the tip region. Only
/// active while our loading portrait path is enabled; otherwise it calls through so vanilla tips render.
pub(crate) unsafe extern "system" fn knowledge_tip_refresh_hook(this: usize) {
    let orig = KNOWLEDGE_TIP_REFRESH_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
        unsafe { f(this) };
    }
    if !gfx_loading_portrait_enabled() {
        return;
    }
    // Suppress: after the movie set the tip, BLANK the title + body handles so no native tip renders --
    // our overlay player-stats text owns the region. Fault-guarded (the SetText core gates on the handle
    // type, so a stale handle is a safe no-op). Runs on the game/render thread.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let null = TITLE_OWNER_SCAN_START_ADDRESS;
        let Ok(base) = game_module_base() else {
            return;
        };
        if base == 0 || base == null || this == 0 || this == null {
            return;
        }
        let settext: unsafe extern "system" fn(usize, usize) =
            unsafe { std::mem::transmute(base + PROFILE_SETTEXT_RVA) };
        let empty = [0u16; 1];
        unsafe {
            settext(this + KNOWLEDGE_TIP_TITLE_HANDLE_OFFSET, empty.as_ptr() as usize);
            settext(this + KNOWLEDGE_TIP_BODY_HANDLE_OFFSET, empty.as_ptr() as usize);
        }
    }));
    KNOWLEDGE_TIP_SUPPRESSED_HITS.fetch_add(1, Ordering::SeqCst);
}

/// Tip-advance "enabled"-predicate detour (er-effects-rs-jsm refinement): while our loading portrait
/// path is active, report the advance action as DISABLED (return 0). The base `MenuWindow::Update`
/// trigger loop then never fires the advance press (the press is a true no-op -- the action's only body
/// is `gotoAndPlay('FadeOut')`, whose downstream tip-refresh we already blank), and the per-update
/// keyguide composer drops the action from the keyguide list, so the "press [button] to advance" prompt
/// never renders. Calls through when the portrait path is off so vanilla tips keep keyguide + press.
pub(crate) unsafe extern "system" fn knowledge_tip_advance_enabled_hook(functor: usize) -> u8 {
    if gfx_loading_portrait_enabled() {
        KNOWLEDGE_TIP_ADVANCE_SUPPRESSED_HITS.fetch_add(1, Ordering::SeqCst);
        return 0;
    }
    let orig = KNOWLEDGE_TIP_ADVANCE_ENABLED_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) -> u8 = unsafe { std::mem::transmute(orig) };
        return unsafe { f(functor) };
    }
    0
}

/// Install the tip-suppression detours on `CS::KnowledgeLoadingScreen`: the tip-refresh no-op (native
/// tip title/body stay blank) and the tip-advance enabled-predicate force-false (keyguide hidden + the
/// advance press inert). One-shot; installed alongside the now-loading observer hooks, before the
/// widget ctor runs.
pub(crate) fn install_tip_suppression_hook() {
    if KNOWLEDGE_TIP_REFRESH_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "stats-text: tip-suppression MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(KNOWLEDGE_TIP_REFRESH_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            knowledge_tip_refresh_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            KNOWLEDGE_TIP_REFRESH_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "stats-text: tip-suppression queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "stats-text: tip-suppression MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    // Second detour in the same apply batch: the advance enabled-predicate. A failure here degrades to
    // tips-blank-but-keyguide-visible, so log and continue rather than abort the batch.
    let mut advance_target = 0usize;
    if let Ok(target2) = game_rva(KNOWLEDGE_TIP_ADVANCE_ENABLED_RVA as u32) {
        match unsafe {
            MhHook::new(
                target2 as *mut c_void,
                knowledge_tip_advance_enabled_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                KNOWLEDGE_TIP_ADVANCE_ENABLED_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                if unsafe { hook.queue_enable() }.is_ok() {
                    advance_target = target2;
                    std::mem::forget(hook);
                } else {
                    append_autoload_debug(format_args!(
                        "stats-text: tip-advance queue_enable failed for 0x{target2:x}"
                    ));
                }
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "stats-text: tip-advance MhHook::new failed: {status:?}"
                ));
            }
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            KNOWLEDGE_TIP_REFRESH_INSTALLED.store(1, Ordering::SeqCst);
            if advance_target != 0 {
                KNOWLEDGE_TIP_ADVANCE_ENABLED_INSTALLED.store(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "stats-text: installed tip-suppression detour 0x{target:x} + advance-disable 0x{advance_target:x} (native tips + keyguide -> our stats text)"
            ));
        }
        status => append_autoload_debug(format_args!(
            "stats-text: tip-suppression MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// Alpha-blend tightly-packed RGBA8 `src` (`sw`x`sh`) OVER `dst` (`dw`x`dh`) at top-left `(x0, y0)`
/// (`src.a`/`1-src.a`). Clips to `dst`. Used to lay the rendered stats text over the head/backbuffer.
#[allow(dead_code)]
pub(crate) fn blend_rgba_over(
    dst: &mut [u8],
    dw: u32,
    dh: u32,
    src: &[u8],
    sw: u32,
    sh: u32,
    x0: i32,
    y0: i32,
) {
    for sy in 0..sh as i32 {
        let dy = y0 + sy;
        if dy < 0 || dy >= dh as i32 {
            continue;
        }
        for sx in 0..sw as i32 {
            let dx = x0 + sx;
            if dx < 0 || dx >= dw as i32 {
                continue;
            }
            let so = ((sy as usize) * (sw as usize) + sx as usize) * 4;
            let a = src[so + 3] as u32;
            if a == 0 {
                continue;
            }
            let dofs = ((dy as usize) * (dw as usize) + dx as usize) * 4;
            if a == 255 {
                dst[dofs..dofs + 4].copy_from_slice(&src[so..so + 4]);
                continue;
            }
            let ia = 255 - a;
            for c in 0..3 {
                dst[dofs + c] = ((src[so + c] as u32 * a + dst[dofs + c] as u32 * ia) / 255) as u8;
            }
            dst[dofs + 3] = 255.min(a + (dst[dofs + 3] as u32 * ia) / 255) as u8;
        }
    }
}
