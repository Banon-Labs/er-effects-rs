// === Candidate A: live head INSIDE the now-loading GFx movie (er-effects-rs-jsm) ==================
//
// GOAL: render the native loading tip text + Gauge_3 bar ABOVE the custom character portrait. The
// 02_903_NowLoading2 movie display list is black-plate(1) < BackImage artwork(3) < Gauge_3(5) <
// tips+keyguide(11), so a portrait delivered as the BackImage artwork texture is layered under the tips
// for free -- whereas the Present-overlay draws after the whole GFx pass and can only ever sit on top.
//
// MECHANISM (BAKE): the forge (loading_bg_replace_bind_hook) already replaces the MENU_Load_ artwork with
// a TPF we build, and GFx DECODES + DISPLAYS that TPF -- this is the proven display path (the "checker"
// seen historically was our OWN forged placeholder, not a game "missing texture" marker). So instead of
// hunting for the exact ID3D12Resource GFx samples (which was unreliable), we composite the live head
// directly INTO the background image the forge builds. GFx shows it; the native tips/bar render on top.
//
// GEOMETRY (single source of truth in constants::NOWLOADING_*): the BackImage places MENU_DummyLoad
// (4096x2048 = 2:1) covering stage (0,0)..(2172,1086); the visible region is the TOP-LEFT 1920x1080 of
// that 2:1 texture. So the forge builds at the 2:1 aspect (no stretch onto the quad) and aspect-cover
// (centre-crop, never stretch) the background + head into the visible top-left sub-rect.
//
// OVERLAY HAND-OFF: the head is baked into a name only when that name is forged AFTER the head is
// captured, and it becomes visible when the movie rotates to that name. Until a baked name is displayed,
// the Present-overlay keeps drawing the head (over the tips) as a BRIDGE so the head is never missing;
// once the displayed name carries the baked head, the overlay demotes and the head renders under the
// tips. Fail-safe: on a load with no post-capture rotation the overlay simply stays engaged (== the prior
// behaviour), never a regression.

/// Candidate A is active exactly when the live look-at head path owns the loading portrait (product
/// default). The head is baked into the forge background; the overlay demotes once it is displayed.
pub(crate) fn gfx_loading_portrait_enabled() -> bool {
    portrait_overlay_enabled()
}

/// Bare `MENU_Load_NNNNN` symbol of the artwork the now-loading movie is CURRENTLY displaying, read from
/// the helper's drawn `replace_tex_info` slot (+0xd8) -> its symbol DLString (+0x10). Falls back to the
/// first forged name (the movie rotates artwork every ~10.9s; following the drawn slot tells us whether
/// the on-screen artwork carries the baked head). `None` only if neither source yields a MENU_Load_ name.
unsafe fn current_displayed_menu_load_name(base: usize) -> Option<String> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let from_helper = (|| {
        let helper =
            unsafe { safe_read_usize(base + RuntimeGlobalRva::NowLoadingSingleton as usize) }?;
        if !valid(helper) {
            return None;
        }
        let rti = unsafe {
            safe_read_usize(helper + core::mem::offset_of!(CSNowLoadingHelperImp, replace_tex_info))
        }?;
        if !valid(rti) {
            return None;
        }
        let (units, _enc) = unsafe { read_dlstring_u16(rti + REPLACE_TEX_INFO_SYMBOL_OFFSET) }?;
        let path = String::from_utf16(&units).ok()?;
        extract_menu_load_tex_name(&path)
    })();
    if from_helper.is_some() {
        return from_helper;
    }
    LOADING_BG_FIRST_TEX_NAME.lock().ok().and_then(|g| g.clone())
}

/// Bytes per RGBA8 texel (local; the gpu_readback `RGBA8_BPP` is not in this module's scope).
const RGBA8_BPP: usize = 4;

/// Names whose forged artwork carries the baked head (so once one is DISPLAYED, the movie already shows
/// the head and the overlay must demote). Populated by `build_baked_loading_bg`, read by the updater. A
/// `Vec` (not `HashSet`) so it is const-constructible in a `static`; only a handful of names per load.
static GFX_PORTRAIT_BAKED_NAMES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Forge replacement texture dimensions (the artwork's 2:1 aspect at `1/NOWLOADING_FORGE_DOWNSCALE`).
fn forge_tex_dims() -> (u32, u32) {
    (
        NOWLOADING_BACKIMAGE_TEX_W / NOWLOADING_FORGE_DOWNSCALE,
        NOWLOADING_BACKIMAGE_TEX_H / NOWLOADING_FORGE_DOWNSCALE,
    )
}

/// Visible top-left sub-rect (in forge-texture pixels) that the 1920x1080 stage actually shows of the
/// 2:1 artwork quad. Derived entirely from the geometry constants -- the artwork quad is
/// `TEX * SPRITE_SCALE` px on stage, anchored at (0,0), so the stage crops it to `STAGE/quad` of the
/// texture. This 16:9 sub-rect is where the background + head are aspect-cover composited.
fn visible_subrect(tw: u32, th: u32) -> (u32, u32) {
    let quad_w = NOWLOADING_BACKIMAGE_TEX_W as f32 * NOWLOADING_BACKIMAGE_SPRITE_SCALE;
    let quad_h = NOWLOADING_BACKIMAGE_TEX_H as f32 * NOWLOADING_BACKIMAGE_SPRITE_SCALE;
    let vis_u = (NOWLOADING_STAGE_W / quad_w).min(1.0);
    let vis_v = (NOWLOADING_STAGE_H / quad_h).min(1.0);
    let vw = ((tw as f32 * vis_u).round() as u32).clamp(1, tw);
    let vh = ((th as f32 * vis_v).round() as u32).clamp(1, th);
    (vw, vh)
}

/// Copy a tightly-packed `sw`x`sh` RGBA8 `src` OPAQUELY into the top-left `sw`x`sh` region of the
/// `dw`x`dh` `dst` (rows are `dw` wide; only the first `sw` px of the first `sh` rows are written).
fn place_top_left(dst: &mut [u8], dw: u32, dh: u32, src: &[u8], sw: u32, sh: u32) {
    let (dw, dh, sw, sh) = (dw as usize, dh as usize, sw as usize, sh as usize);
    let cw = sw.min(dw);
    let ch = sh.min(dh);
    for y in 0..ch {
        let d0 = y * dw * RGBA8_BPP;
        let s0 = y * sw * RGBA8_BPP;
        let n = cw * RGBA8_BPP;
        if d0 + n <= dst.len() && s0 + n <= src.len() {
            dst[d0..d0 + n].copy_from_slice(&src[s0..s0 + n]);
        }
    }
}

/// Alpha-blend a tightly-packed `sw`x`sh` RGBA8 `src` OVER the top-left region of `dst` (`src.a`/`1-src.a`;
/// a transparent src texel leaves the destination background showing). The head is depth-alpha-keyed
/// (background alpha 0), so this lays the head silhouette over the loading background.
fn blend_top_left(dst: &mut [u8], dw: u32, dh: u32, src: &[u8], sw: u32, sh: u32) {
    let (dw, dh, sw, sh) = (dw as usize, dh as usize, sw as usize, sh as usize);
    let cw = sw.min(dw);
    let ch = sh.min(dh);
    for y in 0..ch {
        for x in 0..cw {
            let d = (y * dw + x) * RGBA8_BPP;
            let s = (y * sw + x) * RGBA8_BPP;
            if d + 4 > dst.len() || s + 4 > src.len() {
                continue;
            }
            let a = src[s + 3] as u32;
            if a == 0 {
                continue;
            }
            if a == 255 {
                dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
                continue;
            }
            let ia = 255 - a;
            for c in 0..3 {
                dst[d + c] = ((src[s + c] as u32 * a + dst[d + c] as u32 * ia) / 255) as u8;
            }
            dst[d + 3] = 255;
        }
    }
}

/// Build the forged now-loading background TPF for `symbol` on the live-portrait overlay path: aspect-cover the boot
/// background (or a neutral checker) into the visible top-left sub-rect of a 2:1 texture (centre-crop, no
/// stretch), then -- once the live head has been captured -- alpha-blend the head (also aspect-cover into
/// the same sub-rect) over it. Records `symbol` as head-baked so the updater can demote the overlay when
/// it is displayed. Returns the TPF bytes (`None` on build failure -> caller falls back).
pub(crate) fn build_baked_loading_bg(symbol: &str) -> Option<Vec<u8>> {
    let (tw, th) = forge_tex_dims();
    let (vw, vh) = visible_subrect(tw, th);
    // Base: black texture; the off-screen right/bottom margins stay black (never visible on stage).
    let mut base = vec![0u8; (tw as usize) * (th as usize) * RGBA8_BPP];
    // Background: boot bg aspect-cover -> visible sub-rect (centre-crop = "zoomed in"), else checker.
    let mut bg = boot_bg_image_rgba_clone()
        .map(|(w, h, px)| cover_resample_rgba8(&px, w as u32, h as u32, vw, vh))
        .unwrap_or_else(|| {
            let c = er_tpf::DdsImage::checker(vw, vh, 64, [255, 0, 255, 255], [255, 255, 0, 255]);
            c.pixels
        });
    // Dim to MATCH the first (boot) loading screen, which multiplies the screenshot by 6/16 in
    // `boot_fill_aspect_cover_background` -- so the two loading screens read as the same background.
    for px in bg.chunks_exact_mut(4) {
        px[0] = ((px[0] as u16 * 6) / 16) as u8;
        px[1] = ((px[1] as u16 * 6) / 16) as u8;
        px[2] = ((px[2] as u16 * 6) / 16) as u8;
    }
    place_top_left(&mut base, tw, th, &bg, vw, vh);
    // Head: once captured, aspect-cover into the same visible sub-rect and blend over the background.
    let mut baked = false;
    if PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) != 0 {
        if let Some((hw, hh, hpx)) = LOADING_BG_PORTRAIT_RGBA.lock().ok().and_then(|g| g.clone()) {
            if hw > 0 && hh > 0 {
                let head = cover_resample_rgba8(&hpx, hw, hh, vw, vh);
                blend_top_left(&mut base, tw, th, &head, vw, vh);
                baked = true;
            }
        }
    }
    if baked {
        GFX_PORTRAIT_BAKED.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut g) = GFX_PORTRAIT_BAKED_NAMES.lock() {
            if !g.iter().any(|s| s == symbol) {
                g.push(symbol.to_string());
            }
        }
        if GFX_PORTRAIT_FIRST_LOGGED.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "gfx-loading-portrait: BAKED live head into now-loading background '{symbol}' tex={tw}x{th} visible={vw}x{vh} (2:1 aspect, centre-cropped -- no stretch); shows in-movie under the native tips on the next artwork rotation"
            ));
        }
    }
    let dds = er_tpf::DdsImage {
        width: tw,
        height: th,
        pixels: base,
    }
    .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
    er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok()
}

/// Per-frame (game thread) hook kept for the call site. The overlay hand-off is now driven by the
/// backbuffer PIXEL ORACLE (see `overlay_composite.rs`): the overlay demotes only while the oracle
/// confirms the head is actually on screen, so there is nothing to decide here. (The re-forge experiment
/// was removed: the backbuffer oracle proved a name-once re-forge does NOT re-decode -- head_match=0%.)
pub(crate) unsafe fn maybe_update_gfx_loading_portrait(_base: usize) {}

/// Reset candidate A state at loading-window end (called from `loading_portrait_window_reset`). Clears the
/// per-window head-on-screen state and the baked-name set so the next load starts fresh. (`HEAD_EVER` is a
/// RUN-level product-proof latch and is deliberately NOT reset here.)
pub(crate) fn gfx_loading_portrait_window_reset() {
    GFX_PORTRAIT_DEMOTE_CREDIT.store(0, Ordering::SeqCst);
    GFX_PORTRAIT_HEAD_ON_SCREEN.store(0, Ordering::SeqCst);
    GFX_PORTRAIT_BAKED_DISPLAYED.store(0, Ordering::SeqCst);
    if let Ok(mut g) = GFX_PORTRAIT_BAKED_NAMES.lock() {
        g.clear();
    }
}
