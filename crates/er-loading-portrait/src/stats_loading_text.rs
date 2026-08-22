use crate::prelude::*;

// === Loading-screen player-stats text (er-effects-rs-jsm) =========================================
//
// PIVOT (user 2026-07-06): rather than fight to layer the head UNDER the native tips, we CONTROL the
// surface -- suppress the native loading tips + "press to advance" key guide, and render OUR OWN text
// (the local character's stats) on top of the head in the Present overlay, using the GAME'S OWN menu
// font via `er_gfx::raster::RasterFont`. The font-independent pieces (the unified line FORMAT and the
// CPU text raster) live in the cross-platform `stats_lines` module so the one-layout-everywhere
// guarantee is host-tested (bd er-effects-rs-qic7); this module wires the font capture, the stats
// read, and the overlay composite around them.

// --- Game menu font: captured at runtime from the game's own Scaleform file-open, or from an env
// --- diagnostic .gfx on disk. NOTHING is embedded (per the no-game-derived-binaries rule).
/// Raw captured `font.gfx` bytes (copied out of the game's Scaleform MemoryFile in the file-open hook).
static MENU_FONT_GFX_CAPTURED: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
/// The parsed, cached menu font (built once from the captured `font.gfx` bytes).
static MENU_FONT_RASTER: std::sync::OnceLock<er_gfx::raster::RasterFont> =
    std::sync::OnceLock::new();

/// Capture the game's menu font from the Scaleform file-open hook. Reads the returned MemoryFile's raw
/// GFX payload (same guarded read `title_05_000_swap_to_stripped` uses) and stores a COPY (never retains
/// the game pointer). Called for any file-open whose URL looks like the menu font; one-shot.
///
/// # Safety
///
/// `base` must be the running `eldenring.exe` image base: the candidate is accepted only
/// if its vtable equals `base + SCALEFORM_MEMORY_FILE_VTABLE_RVA`, and a wrong base makes
/// that check meaningless rather than merely wrong.
///
/// `file` needs no precondition for the header reads (all guarded), but once the vtable,
/// length and both end bytes have been probed, the payload is copied with a single
/// `slice::from_raw_parts` over up to 64 MiB -- an UNGUARDED bulk read. The caller must
/// therefore call this only from the Scaleform file-open hook, while the `MemoryFile` the
/// game just produced is still alive and its buffer is not being freed on another thread.
pub unsafe fn capture_menu_font_gfx(base: usize, file: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if MENU_FONT_GFX_CAPTURED.get().is_some()
        || base == 0
        || base == null
        || file == 0
        || file == null
    {
        return;
    }
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return;
    }
    let data = unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
    let len = unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
    if data == 0 || !(8..=64 * 1024 * 1024).contains(&len) {
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

/// The game's menu font, parsed + cached from the runtime file-open capture of `font.gfx`. `None` until
/// the capture has happened and the font parses (product path only -- no env crutch).
pub fn menu_font() -> Option<&'static er_gfx::raster::RasterFont> {
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

/// Read the local character's stats for the loading screen. `None` if GameDataMan is not up. Prefers
/// sources valid pre-load; guards every read.
///
/// # Safety
///
/// Every game access is a guarded read or a host-seam call, so no argument-level
/// precondition exists -- an absent GameDataMan or an unpopulated slot returns `None`.
///
/// Game thread only. The values it reads (GameDataMan, the slot's ProfileSummary record,
/// the live PlayerGameData) are the ones the save deserialize rewrites; sampling them from
/// another thread yields a torn mix of the outgoing and incoming character rather than a
/// fault, which is exactly the wrong-stats bug the ownership checks here exist to prevent.
pub unsafe fn read_loading_screen_stats() -> Option<LoadingScreenStats> {
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
    // ProfileSummary record: name / level (populated before the load). The record also carries a
    // playtime at PROFILE_SUMMARY_PLAYTIME_OFFSET, which this panel no longer shows (user
    // 2026-08-07: line 2 is RL + WL); that offset is still live for the save-slot writer.
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(0);
    let mut name = String::new();
    let mut level = 0i32;
    if valid(summary) {
        let rec = profile_summary_record_address(summary, slot_u);
        let (units, len) = unsafe { read_utf16_name_units(rec) };
        name = String::from_utf16_lossy(&units[..len]);
        level = unsafe { safe_read_i32(rec + PROFILE_SUMMARY_LEVEL_OFFSET) }.unwrap_or(0);
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
        ll > 0 && pgd_level > 0 && pgd_level == level && String::from_utf16_lossy(&ln[..ll]) == name
    });
    let (attributes, max_hp, max_fp, max_stamina, weapon_level, attr_source_live) =
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
                // Weapon level off the SAME validated PGD as the attributes and vitals: the pointer has
                // already been proved to own the loading slot's character (name + level match the
                // slot-scoped ProfileSummary record), so this cannot pair one character's stats with
                // another's `WL`. An implausible byte reads as unknown rather than being rendered.
                unsafe { safe_read_u8(pgd + PGD_MATCHING_WEAPON_LEVEL_E2_OFFSET) }
                    .filter(|&v| v <= PGD_MATCHING_WEAPON_LEVEL_MAX),
                true,
            )
        } else {
            let base = game_module_base().unwrap_or(null);
            if valid(base) {
                let _ = unsafe { ensure_profile_slot_stats_cached(base) };
            }
            let attrs = profile_slot_attributes(slot).unwrap_or([0; 8]);
            // Unified layout (bd er-effects-rs-qic7): pre-mount, the effective max vitals come
            // from the save slot's serialized PlayerGameData (STORED MaxHealth/MaxFP/MaxSP ==
            // runtime current_max_*; located by the same rune-level-invariant scan as the
            // attributes) so the boot loading screen renders the SAME five-line panel as
            // subsequent live loads. [0,0,0] (rendered as `--`) only when the save is unreadable.
            let [hp, fp, stam] = profile_slot_vitals(slot).unwrap_or([0; 3]);
            // Same `.sl2` slot cache the attributes and vitals come from, so the whole panel describes
            // one decode of one slot. `er_save_loader::stats` already rejects an implausible byte, so
            // `None` here genuinely means "not decodable" and renders as `--`. There is no live
            // fallback: pre-deserialize PlayerGameData is the game's default template, whose weapon
            // level belongs to no character on screen.
            let wl = profile_slot_weapon_level(slot);
            (attrs, hp, fp, stam, wl, false)
        };
    Some(LoadingScreenStats {
        name,
        level,
        attributes,
        max_hp,
        max_fp,
        max_stamina,
        weapon_level,
        attr_source_live,
    })
}

/// The rendered loading-screen stats text, keyed by the exact display `lines` it renders. The game
/// thread rebuilds it whenever the loading slot's lines differ (character switch, record->live
/// upgrade); the render thread composites it. ONE mutex guards bitmap + key together so a window
/// reset racing a build can never strand a key without its bitmap (which would suppress rebuilds and
/// blank the text for the whole window).
pub struct StatsTextCache {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// The exact lines the bitmap renders -- the rebuild key.
    pub lines: Vec<String>,
}
pub static STATS_TEXT_CACHE: std::sync::Mutex<Option<StatsTextCache>> = std::sync::Mutex::new(None);

/// Screen-resolution text bitmap cache for the Present overlay. The stats lines still come from the
/// game thread (safe slot/stat reads), but the final bitmap is sized from the actual backbuffer so the
/// text is no longer baked into, and then upscaled with, the lower-resolution animated portrait RT.
struct StatsTextScreenCache {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    lines: Vec<String>,
    screen_max_dim: u32,
    version: usize,
}
static STATS_TEXT_SCREEN_CACHE: std::sync::Mutex<Option<StatsTextScreenCache>> =
    std::sync::Mutex::new(None);
pub use er_telemetry::counters::STATS_TEXT_SCREEN_VERSION;

/// Cumulative stats-bitmap build count (telemetry oracle `oracle_stats_text_built`; never reset).
pub use er_telemetry::counters::STATS_TEXT_BUILT;
/// `(name, level, live)` of the last logged build -- gates the debug log so repeat builds of the same
/// identity don't spam it, while identity changes (new character, record->live upgrade) still log.
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
///
/// # Safety
///
/// Game thread only, for the same reason as `read_loading_screen_stats`, which it calls:
/// the stats it rasterises must come from one consistent sample of the loading slot.
///
/// It performs no unguarded game access of its own -- the rasteriser works on the captured
/// font copy and on plain Rust data -- so there is no address precondition.
pub unsafe fn maybe_build_stats_text() {
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
    // PROPORTIONAL FONT SIZE (user 2026-07-06): the stats text is composited INTO the head render target,
    // which is then aspect-cover UPSCALED to the backbuffer -- so a FIXED-pixel em_px changes its on-screen
    // size whenever the render resolution changes (halving the RT 2056->1028 doubled the on-screen text).
    // Size the font as a constant FRACTION of the RT height instead (shared consts in `stats_lines`, so
    // every build path uses the SAME em sizing). rt_dim is the offscreen size we patch the portrait RT
    // to (confirmed == oracle_ls_portrait_h).
    let rt_dim = (PROFILE_OFFSCREEN_SIZE_TARGET & 0xffff_ffff) as f32;
    let em_px = rt_dim * (STATS_TEXT_EM_PX_AT_REF_RT / STATS_TEXT_REF_RT_DIM);
    let (w, h, rgba) = render_lines_to_rgba(font, &lines, em_px, [238, 228, 202, 255]);
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

/// Return a stats-text bitmap sized for the actual backbuffer. This is the high-resolution product path:
/// the animated character stays at the current portrait RT resolution, while text is rasterized directly
/// at screen scale and drawn as a second Present-overlay texture. The returned `version` is stable while
/// both the display lines and requested backbuffer scale are unchanged, so the D3D texture uploads only on
/// content/size changes.
pub fn stats_text_screen_bitmap(screen_max_dim: u32) -> Option<(u32, u32, Vec<u8>, usize)> {
    if screen_max_dim == 0 {
        return None;
    }
    let lines = STATS_TEXT_CACHE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| c.lines.clone()))?;
    if lines.is_empty() {
        return None;
    }
    if let Some(cached) = STATS_TEXT_SCREEN_CACHE.lock().ok().and_then(|g| {
        g.as_ref()
            .filter(|c| c.screen_max_dim == screen_max_dim && c.lines == lines)
            .map(|c| (c.w, c.h, c.rgba.clone(), c.version))
    }) {
        return Some(cached);
    }
    let font = menu_font()?;
    // Same shared em-sizing consts as the RT build (`stats_lines`): one em everywhere.
    let em_px = screen_max_dim as f32 * (STATS_TEXT_EM_PX_AT_REF_RT / STATS_TEXT_REF_RT_DIM);
    let (w, h, rgba) = render_lines_to_rgba(font, &lines, em_px, [238, 228, 202, 255]);
    if w == 0 || h == 0 {
        return None;
    }
    let version = STATS_TEXT_SCREEN_VERSION.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut g) = STATS_TEXT_SCREEN_CACHE.lock() {
        *g = Some(StatsTextScreenCache {
            w,
            h,
            rgba: rgba.clone(),
            lines,
            screen_max_dim,
            version,
        });
    }
    Some((w, h, rgba, version))
}

/// Cheap presence check: true once the game-thread build (`maybe_build_stats_text`) has produced
/// loading-screen stats lines. The native isolated overlay uses this as a full-frame/composite gate so
/// it never pays the screen-scale raster (`stats_text_screen_bitmap`) just to decide whether to show
/// anything. Only the `lines` matter -- the screen bitmap is re-rastered from them at screen scale.
pub fn stats_text_available() -> bool {
    STATS_TEXT_CACHE
        .lock()
        .ok()
        .is_some_and(|g| g.as_ref().is_some_and(|c| !c.lines.is_empty()))
}

/// Reset the per-load stats-text cache so the next load starts from a clean (no-text) frame and its
/// first build logs. Correctness does NOT depend on this reset: the content key in
/// `maybe_build_stats_text` rebuilds on any line change even if a post-reset tick re-caches the old
/// character. `STATS_TEXT_BUILT` is a cumulative oracle and is deliberately not reset.
pub fn stats_text_window_reset() {
    if let Ok(mut g) = STATS_TEXT_CACHE.lock() {
        *g = None;
    }
    if let Ok(mut g) = STATS_TEXT_SCREEN_CACHE.lock() {
        *g = None;
    }
    if let Ok(mut g) = STATS_TEXT_LOGGED.lock() {
        *g = None;
    }
}

/// Tip-refresh detour: NO-OP the original (er-effects-rs-jsm PIVOT) so the native tip title/body are never
/// set and the `Main` tip clip stays faded out -- our overlay player-stats text owns the tip region. Only
/// active while our loading portrait path is enabled; otherwise it calls through so vanilla tips render.
///
/// # Safety
///
/// Do NOT call this directly. It is the detour body MinHook installs over the game's knowledge-tip
/// refresh, so it may only be entered by that patched call site, on the game thread that made the
/// call, with the arguments and `extern "system"` ABI the original declares.
///
/// It calls the saved original through a `transmute` of the trampoline address held in
/// `KNOWLEDGE_TIP_REFRESH_ORIG`; that static must hold the trampoline this detour was installed
/// with, or the call transfers to an arbitrary address.
///
/// After calling through, it blanks the tip's title/body text handles. Those writes go
/// through the game's own SetText core, which gates on the handle type, so a stale handle
/// is a no-op -- but they still assume `this` is the tip window the detour was installed
/// for.
pub unsafe extern "system" fn knowledge_tip_refresh_hook(this: usize) {
    let orig = KNOWLEDGE_TIP_REFRESH_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
        unsafe { f(this) };
    }
    if !portrait_overlay_enabled() {
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
            settext(
                this + KNOWLEDGE_TIP_TITLE_HANDLE_OFFSET,
                empty.as_ptr() as usize,
            );
            settext(
                this + KNOWLEDGE_TIP_BODY_HANDLE_OFFSET,
                empty.as_ptr() as usize,
            );
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
///
/// # Safety
///
/// Do NOT call this directly. It is the detour body MinHook installs over the game's tip-advance
/// enabled predicate, so it may only be entered by that patched call site, on the game thread that
/// made the call, with the arguments and `extern "system"` ABI the original declares.
///
/// It calls the saved original through a `transmute` of the trampoline address held in
/// `KNOWLEDGE_TIP_ADVANCE_ENABLED_ORIG`; that static must hold the trampoline this detour was
/// installed with, or the call transfers to an arbitrary address.
///
/// When the portrait path is enabled it returns 0 without touching `functor` at all, so
/// that argument is only dereferenced by the original on the pass-through path.
pub unsafe extern "system" fn knowledge_tip_advance_enabled_hook(functor: usize) -> u8 {
    if portrait_overlay_enabled() {
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
pub fn install_tip_suppression_hook() {
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
            // The handle is deliberately dropped here without ceremony: `MhHook` is three raw
            // pointers with no `Drop`, and MinHook owns the installed detour keyed by target
            // address -- so letting the handle go does NOT uninstall the hook.
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
                    // The handle is deliberately dropped here without ceremony: `MhHook` is three raw
                    // pointers with no `Drop`, and MinHook owns the installed detour keyed by target
                    // address -- so letting the handle go does NOT uninstall the hook.
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

/// A tightly-packed RGBA8 source image: the pixel bytes together with the width/height they are
/// laid out for. Carrying the three as one value keeps the blend's parameter list honest -- the
/// bytes and the dimensions that describe them cannot be passed out of step.
pub struct Rgba8Src<'a> {
    pub px: &'a [u8],
    pub w: u32,
    pub h: u32,
}

/// Alpha-blend tightly-packed RGBA8 `src` OVER `dst` (`dw`x`dh`) at top-left `(x0, y0)`
/// (`src.a`/`1-src.a`). Clips to `dst`. Used to lay the rendered stats text over the head/backbuffer.
pub fn blend_rgba_over(dst: &mut [u8], dw: u32, dh: u32, src: Rgba8Src<'_>, x0: i32, y0: i32) {
    let Rgba8Src {
        px: src,
        w: sw,
        h: sh,
    } = src;
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
