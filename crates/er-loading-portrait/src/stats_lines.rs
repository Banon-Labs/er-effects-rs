//! Pure (host-testable) loading-screen stats layer: the line FORMAT and the CPU text
//! raster, split out of the windows-only `stats_loading_text` module (bd
//! er-effects-rs-qic7) so the ONE unified layout is provable off-target.
//!
//! UNIFIED LAYOUT (user decision 2026-07-30): the boot/autoload loading screen and every
//! subsequent load screen render the SAME five-line panel -- name; RL+WL;
//! HP/FP/Stamina; VIG..STR; DEX..ARC. The old divergence (the HP line was dropped when
//! live PlayerGameData was not yet validated, 361x148 vs 361x184) is gone:
//! [`format_stats_lines`] always emits the HP line, with the stored save-slot vitals
//! pre-mount and `--` placeholders only when a vital is genuinely unknown, so the bitmap
//! geometry never jumps when live data arrives mid-screen.
//!
//! Line 2 carries the rune level and the matchmaking weapon level (`RL 9    WL 12`), the
//! same pair the ProfileSelect row header shows (user 2026-08-07). It replaced the play
//! time, which was the one value on the panel that ticked every second: each tick changed
//! the content key, rebuilt the bitmap, and re-uploaded the screen-scale texture for a
//! number nobody reads mid-load.

/// The local character's loading-screen stats (er-effects-rs-jsm). Read from the
/// loading-screen-safe ProfileSummary record (name/level) + live PlayerGameData when up
/// (attributes, HP/FP/Stamina, weapon level), falling back to the `.sl2` slot cache
/// (attributes, stored max vitals AND weapon level) pre-load.
pub struct LoadingScreenStats {
    pub name: String,
    pub level: i32,
    pub attributes: [i32; 8], // VIG,MND,END,STR,DEX,INT,FAI,ARC
    pub max_hp: u32,
    pub max_fp: u32,
    pub max_stamina: u32,
    /// Highest weapon upgrade level (`PlayerGameData::matching_weapon_level`), or `None`
    /// when no source could supply it. `Some(0)` is a REAL answer -- a character with
    /// nothing upgraded -- and is deliberately distinct from `None` ("we do not know").
    pub weapon_level: Option<u8>,
    pub attr_source_live: bool,
}

/// PROPORTIONAL FONT SIZE (user 2026-07-06): 48px was tuned at the 2056 RT, so
/// `em_px = dim * STATS_TEXT_EM_PX_AT_REF_RT / STATS_TEXT_REF_RT_DIM` keeps the text the
/// same on-screen size at any render resolution. Shared by the RT-composited build and
/// the screen-scale Present-overlay build so both use the SAME em sizing.
pub const STATS_TEXT_EM_PX_AT_REF_RT: f32 = 48.0;
/// The reference RT dimension the 48px em was tuned at.
pub const STATS_TEXT_REF_RT_DIM: f32 = 2056.0;

/// The weapon level for display: the number, or `--` while genuinely unknown. Same
/// placeholder idiom as [`fmt_vital`], but keyed on `None` rather than 0, because `WL 0`
/// is a true statement about a character who has upgraded nothing.
///
/// The `WL` token itself is NEVER dropped -- unlike the ProfileSelect row header, whose
/// whole `WL` group disappears when the value is unknown. That panel is a single line of
/// prose; this one is a fixed five-line block whose bitmap is composited at a stable
/// height, so a token appearing when live data arrives mid-screen would move the text.
fn fmt_weapon_level(v: Option<u8>) -> String {
    match v {
        Some(v) => v.to_string(),
        None => "--".to_string(),
    }
}

/// A max-vital value for display: the stored/live number, or `--` while genuinely
/// unknown (a real character's stored maxima are always > 0, so 0 == "not read yet";
/// the value upgrades in place when the save cache or live PGD provides it).
fn fmt_vital(v: u32) -> String {
    if v > 0 {
        v.to_string()
    } else {
        "--".to_string()
    }
}

/// Lay the stats out as display lines -- the ONE layout used on every loading screen
/// (boot/autoload AND subsequent loads): name; RL+WL; HP/FP/Stamina; the 8 attributes
/// over two lines. The HP line is UNCONDITIONAL (bd er-effects-rs-qic7): the line count
/// (and so the bitmap height) is identical whether the vitals came from the save slot,
/// live PlayerGameData, or are still unknown (`--`).
pub fn format_stats_lines(st: &LoadingScreenStats) -> Vec<String> {
    let a = &st.attributes;
    let name = if st.name.trim().is_empty() {
        "Tarnished".to_string()
    } else {
        st.name.clone()
    };
    vec![
        name,
        format!(
            "RL {}    WL {}",
            st.level,
            fmt_weapon_level(st.weapon_level)
        ),
        format!(
            "HP {}    FP {}    Stamina {}",
            fmt_vital(st.max_hp),
            fmt_vital(st.max_fp),
            fmt_vital(st.max_stamina)
        ),
        format!("VIG {}   MND {}   END {}   STR {}", a[0], a[1], a[2], a[3]),
        format!("DEX {}   INT {}   FTH {}   ARC {}", a[4], a[5], a[6], a[7]),
    ]
}

/// Parse `.gfx` bytes and build a `RasterFont` from the DefineFont3 with the most glyphs
/// (best ASCII coverage), recursing into DefineSprite. `None` if no font tag decodes.
/// (Pure bytes -> font; the runtime capture and the host tests share this exact path.)
pub fn build_menu_font_from_gfx(bytes: &[u8]) -> Option<er_gfx::raster::RasterFont> {
    let movie = er_gfx::Movie::parse(bytes).ok()?;
    fn best_font<'a>(
        tags: &'a [er_gfx::Tag],
        best: &mut Option<&'a er_gfx::Tag>,
        best_n: &mut usize,
    ) {
        for t in tags {
            if let er_gfx::Tag::DefineFont3 { glyphs, codes, .. } = t
                && glyphs.len() == codes.len()
                && glyphs.len() > *best_n
            {
                *best_n = glyphs.len();
                *best = Some(t);
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

/// Render a stack of left-aligned text `lines` to a tightly-packed RGBA8 bitmap using the
/// parsed game menu font `font`, at `em_px` glyph height. Each glyph's coverage becomes
/// `color` (RGB = color.rgb, alpha = coverage * color.a / 255), so the result composites
/// straight over the head. Returns `(width, height, rgba)` sized to the glyphs' bounding
/// box plus a 1px pad, or `(0,0,vec![])` if nothing rendered. Pure CPU, no game state;
/// safe to call from any thread.
pub fn render_lines_to_rgba(
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
                placed.push(Placed { bmp, x: gx, y: gy });
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
        blit(
            &mut rgba,
            p,
            dx0 + SHADOW,
            dy0 + SHADOW,
            [0, 0, 0, color[3]],
        );
    }
    for p in &placed {
        let dx0 = (p.x - min_x + pad).round() as i32;
        let dy0 = (p.y - min_y + pad).round() as i32;
        blit(&mut rgba, p, dx0, dy0, color);
    }
    (w, h, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values of a real character (9-Menace slot 0, oracle ground truth) as the LIVE
    /// source would report them.
    fn live_stats() -> LoadingScreenStats {
        LoadingScreenStats {
            name: "Menace".to_string(),
            level: 9,
            attributes: [15, 10, 11, 14, 13, 9, 9, 7],
            max_hp: 870,
            max_fp: 121,
            max_stamina: 115,
            weapon_level: Some(12),
            attr_source_live: true,
        }
    }

    /// The same character as the PRE-MOUNT source reports it (save-slot cache: same
    /// stored values, `attr_source_live == false`).
    fn save_slot_stats() -> LoadingScreenStats {
        LoadingScreenStats {
            attr_source_live: false,
            ..live_stats()
        }
    }

    /// The degraded pre-mount case: the `.sl2` was unreadable, vitals genuinely unknown.
    fn unknown_vitals_stats() -> LoadingScreenStats {
        LoadingScreenStats {
            max_hp: 0,
            max_fp: 0,
            max_stamina: 0,
            attr_source_live: false,
            ..live_stats()
        }
    }

    /// The same degradation for the weapon level alone: no live PGD and no decodable
    /// `matchmakingWeaponLevel` byte in the slot cache.
    fn unknown_weapon_level_stats() -> LoadingScreenStats {
        LoadingScreenStats {
            weapon_level: None,
            attr_source_live: false,
            ..live_stats()
        }
    }

    /// The unified layout: both data sources produce the IDENTICAL five-line structure
    /// (bd er-effects-rs-qic7 -- the old live=false variant dropped the HP line).
    #[test]
    fn both_sources_produce_identical_line_structure() {
        let live = format_stats_lines(&live_stats());
        let save = format_stats_lines(&save_slot_stats());
        assert_eq!(live.len(), 5, "unified layout is exactly five lines");
        assert_eq!(
            live, save,
            "same character values must format identically regardless of source"
        );
        assert_eq!(live[1], "RL 9    WL 12");
        assert_eq!(live[2], "HP 870    FP 121    Stamina 115");
    }

    /// Line 2 is the rune level and the matchmaking weapon level -- the same pair the
    /// ProfileSelect row header shows -- and carries no play time (user 2026-08-07).
    #[test]
    fn the_level_line_shows_rl_and_wl_and_no_play_time() {
        let lines = format_stats_lines(&live_stats());
        assert_eq!(lines[1], "RL 9    WL 12");
        assert!(
            !lines.iter().any(|l| l.contains("Time")),
            "play time must not appear anywhere on the panel: {lines:?}"
        );
        // `WL 0` is a real character with nothing upgraded, not a missing value.
        let fresh = LoadingScreenStats {
            weapon_level: Some(0),
            ..live_stats()
        };
        assert_eq!(format_stats_lines(&fresh)[1], "RL 9    WL 0");
    }

    /// An unknown weapon level still renders the `WL` token with the `--` placeholder.
    /// Dropping the token (as the one-line ProfileSelect header does) would re-flow this
    /// panel's fixed block the moment live PlayerGameData validated mid-screen.
    #[test]
    fn unknown_weapon_level_keeps_the_wl_token() {
        let lines = format_stats_lines(&unknown_weapon_level_stats());
        assert_eq!(lines.len(), 5, "the panel is still five lines");
        assert_eq!(lines[1], "RL 9    WL --");
        assert!(
            lines[1].contains("WL"),
            "the WL token must never be dropped"
        );
        // Every other line is identical to the known-weapon-level build.
        let known = format_stats_lines(&live_stats());
        for i in [0usize, 2, 3, 4] {
            assert_eq!(lines[i], known[i]);
        }
    }

    /// Even when the vitals are genuinely unknown (no `.sl2`, no live PGD), the HP line
    /// is still PRESENT (placeholders) so the line count -- and the bitmap height --
    /// never changes when live data arrives mid-screen.
    #[test]
    fn unknown_vitals_keep_the_five_line_layout() {
        let lines = format_stats_lines(&unknown_vitals_stats());
        assert_eq!(lines.len(), 5, "HP line must not be dropped");
        assert_eq!(lines[2], "HP --    FP --    Stamina --");
        // Every other line is identical to the known-vitals build.
        let known = format_stats_lines(&live_stats());
        assert_eq!(lines[0], known[0]);
        assert_eq!(lines[1], known[1]);
        assert_eq!(lines[3], known[3]);
        assert_eq!(lines[4], known[4]);
    }

    /// The game's own menu font from the local extraction corpus, or `None` (SKIP) when
    /// absent. Env-overridable like `ER_GFX_CORPUS_ROOT` (the default embeds this
    /// machine's extraction root; game-derived bytes are never versioned).
    fn corpus_menu_font() -> Option<er_gfx::raster::RasterFont> {
        let path = match std::env::var("ER_FONT_GFX_PATH") {
            Ok(v) if !v.trim().is_empty() => std::path::PathBuf::from(v),
            _ => {
                let home = std::env::var("HOME").ok()?;
                std::path::PathBuf::from(home)
                    .join("er-extract/LOOK_HERE_ALL_ASSETS_20260713/font/eu_std/font.gfx")
            }
        };
        if !path.exists() {
            eprintln!(
                "SKIP: menu font {} not present; geometry test skipped",
                path.display()
            );
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        build_menu_font_from_gfx(&bytes)
    }

    /// Bitmap-geometry proof (corpus-gated): with the real menu font, the save-sourced
    /// and live-sourced builds of the same character are byte-identical, and the
    /// unknown-vitals build has the SAME height (the geometry that used to jump 148->184
    /// when the HP line appeared).
    #[test]
    fn bitmap_geometry_is_identical_for_both_sources() {
        let Some(font) = corpus_menu_font() else {
            return;
        };
        let em = 48.0;
        let color = [238, 228, 202, 255];
        let live = render_lines_to_rgba(&font, &format_stats_lines(&live_stats()), em, color);
        let save = render_lines_to_rgba(&font, &format_stats_lines(&save_slot_stats()), em, color);
        assert!(live.0 > 0 && live.1 > 0, "live build must render");
        assert_eq!(
            (live.0, live.1),
            (save.0, save.1),
            "save-sourced and live-sourced bitmaps must have identical geometry"
        );
        assert_eq!(
            live.2, save.2,
            "identical lines must render identical pixels"
        );
        let unknown = render_lines_to_rgba(
            &font,
            &format_stats_lines(&unknown_vitals_stats()),
            em,
            color,
        );
        assert_eq!(
            live.1, unknown.1,
            "placeholder vitals must not change the bitmap height"
        );
        let no_wl = render_lines_to_rgba(
            &font,
            &format_stats_lines(&unknown_weapon_level_stats()),
            em,
            color,
        );
        assert_eq!(
            live.1, no_wl.1,
            "a placeholder weapon level must not change the bitmap height"
        );
    }
}
