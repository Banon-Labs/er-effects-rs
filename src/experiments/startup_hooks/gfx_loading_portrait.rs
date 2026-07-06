// === Candidate A: live head INSIDE the now-loading GFx movie (er-effects-rs-jsm) ==================
//
// The Present-overlay (gpu_readback/overlay_composite.rs) draws the live look-at head over the WHOLE
// backbuffer AFTER the entire GFx pass, so it necessarily paints over the movie's own tip text and
// Gauge_3 loading bar. To get the native tips/bar to render ABOVE the portrait, the head must live
// INSIDE the movie: the 02_903_NowLoading2 display list is black-plate(1) < BackImage artwork(3) <
// Gauge_3(5) < tips+keyguide(11), so a portrait delivered as the artwork texture is layered under the
// tips/bar for free (bd tooltip-above-portrait-VERDICT-2026-07-05).
//
// MECHANISM (bd gfx-decoded-tex-deterministic-resolve-2026-07-05): the texture GFx samples for a
// MENU_Load_NNNNN background is a `CS::CSTextureImage` in the Scaleform tex repository name-map. We
// resolve it BY NAME (the exact bare symbol the forge used) via the game's own resolver, take its
// GFx-sampled HAL texture at +0x10, and per-frame CopyTextureRegion the live head into that resource.
// Object identity + descriptors stay fixed (no gx-pointer swap -> no vkd3d sampler fault), and the
// resolve is by name (no D3D12 global-object scan -> nothing to race the teardown). The forge already
// bound our TPF for every MENU_Load_ symbol, so this simply refreshes the DISPLAYED texture's content.
//
// FAIL-OPEN: every failure (repo not up, resolve miss, dim/format reject, upload error) leaves the
// Present-overlay in charge (it only demotes while this path is actively succeeding), so the proven
// working overlay is never regressed -- this can only add the correct native layering or no-op.

/// Candidate A is active exactly when the live look-at head path owns the loading portrait (product
/// default). It replaces the overlay's display role: the head goes into the movie and the overlay
/// yields (demotes) while this succeeds.
pub(crate) fn gfx_loading_portrait_enabled() -> bool {
    portrait_lookat_enabled()
}

/// Bare `MENU_Load_NNNNN` symbol of the artwork the now-loading movie is CURRENTLY displaying, read from
/// the helper's drawn `replace_tex_info` slot (+0xd8) -> its symbol DLString (+0x10). Falls back to the
/// first forged name (the movie rotates artwork every ~10.9s; following the drawn slot keeps the head on
/// whichever texture is on screen). `None` only if neither source yields a MENU_Load_ name.
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
        let (units, _enc) =
            unsafe { read_dlstring_u16(rti + REPLACE_TEX_INFO_SYMBOL_OFFSET) }?;
        let path = String::from_utf16(&units).ok()?;
        extract_menu_load_tex_name(&path)
    })();
    if from_helper.is_some() {
        return from_helper;
    }
    // Fallback: the first forged (initially displayed) name captured by the forge hook.
    LOADING_BG_FIRST_TEX_NAME.lock().ok().and_then(|g| g.clone())
}

/// Drop one owned reference on a `CSTextureImage` via the game's Scaleform RefCountImpl Release. MUST be
/// called on the game thread (a refcount reaching 0 frees through the object's vtable, touching Scaleform
/// state). Fault-guarded; a resolve failure or bad pointer is a silent no-op.
unsafe fn release_gfx_texture_image(base: usize, img: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if img == 0 || img == null {
        return;
    }
    let Ok(rel) = game_rva(SCALEFORM_REFCOUNT_RELEASE_RVA as u32) else {
        return;
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(rel) };
        unsafe { f(img) };
    }));
}

/// Resolve the DISPLAYED `CSTextureImage` for `name` from the Scaleform tex repository and return
/// `(img, hal_texture)`. `img` is AddRef'd (owned by the caller -> Release when done). NULL-CHECKS the
/// repository singleton first: the resolver PANICS (non-returning) on a null repo, so a not-yet-up repo
/// must fail closed here. Returns `None` on repo-null / resolver-miss / bad HAL pointer.
unsafe fn resolve_displayed_gfx_texture(base: usize, name: &str) -> Option<(usize, usize)> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    // Fail closed if graphics/repos are not up (else the resolver DLPanics = crash).
    let repo = unsafe { safe_read_usize(base + GLOBAL_SCALEFORM_TEX_REPOSITORY_RVA) }.unwrap_or(0);
    if !valid(repo) {
        GFX_PORTRAIT_LAST_ERROR.store(GFX_PORTRAIT_ERR_REPO_NULL, Ordering::SeqCst);
        return None;
    }
    let Ok(resolver) = game_rva(SCALEFORM_TEX_RESOLVE_RVA as u32) else {
        return None;
    };
    let name_z: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
    let mut out: usize = 0;
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // FUN_140d7c9f0(param1_IGNORED, out, name): loads the repo singleton itself, stores an AddRef'd
        // CSTextureImage* into *out (hit = existing, miss = builds via the GetResCap bridge).
        let f: unsafe extern "system" fn(usize, *mut usize, *const u16) -> usize =
            unsafe { std::mem::transmute(resolver) };
        unsafe { f(0, &mut out, name_z.as_ptr()) };
    }))
    .is_ok();
    if !ok || !valid(out) {
        GFX_PORTRAIT_LAST_ERROR.store(GFX_PORTRAIT_ERR_RESOLVE_ZERO, Ordering::SeqCst);
        if valid(out) {
            unsafe { release_gfx_texture_image(base, out) };
        }
        return None;
    }
    let hal = unsafe { safe_read_usize(out + CS_TEXTURE_IMAGE_HAL_TEX_OFFSET) }.unwrap_or(0);
    if !valid(hal) {
        GFX_PORTRAIT_LAST_ERROR.store(GFX_PORTRAIT_ERR_BAD_HAL, Ordering::SeqCst);
        unsafe { release_gfx_texture_image(base, out) };
        return None;
    }
    unsafe { dump_gfx_texture_layout(base, out, hal) };
    Some((out, hal))
}

/// One-time read-only field dump of the resolved `CSTextureImage` and its `+0x10` HAL texture, so the
/// exact offset from the HAL texture to its `ID3D12Resource` can be read off the debug log (the RE leaf
/// that `find_d3d12_resource`'s bounded BFS is failing to reach -- bd gfx-decoded-tex-deterministic-
/// resolve-2026-07-05). For each qword field it logs the pointee's game-image RVA (if in the EXE) and
/// whether the pointee's own vtable lands in a d3d12 module (= a candidate `ID3D12Resource`).
static GFX_PORTRAIT_LAYOUT_DUMPED: AtomicUsize = AtomicUsize::new(0);
unsafe fn dump_gfx_texture_layout(base: usize, img: usize, hal: usize) {
    if GFX_PORTRAIT_LAYOUT_DUMPED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let d3d: Vec<(usize, usize)> = [
        b"d3d12core.dll\0".as_slice(),
        b"d3d12.dll\0".as_slice(),
        b"dxgi.dll\0".as_slice(),
    ]
    .iter()
    .filter_map(|n| unsafe { module_range(n) })
    .collect();
    let in_d3d = |vt: usize| d3d.iter().any(|&(lo, hi)| lo <= vt && vt < hi);
    let read = |p: usize| unsafe { safe_read_usize(p) }.unwrap_or(0);
    let rva = |vt: usize| if vt >= base { vt - base } else { usize::MAX };
    let img_vt = read(img);
    let img_w = unsafe { safe_read_i32(img + 0x2c) }.unwrap_or(0);
    let img_h = unsafe { safe_read_i32(img + 0x30) }.unwrap_or(0);
    append_autoload_debug(format_args!(
        "gfx-tex-dump: CSTextureImage=0x{img:x} vt_rva=0x{:x} w={img_w} h={img_h} hal=0x{hal:x} (expect w/h == forged FORGE_HEAD_TEX_DIM)",
        rva(img_vt)
    ));
    // Walk the HAL texture's fields; for each pointer field, log its pointee's vtable RVA + d3d membership,
    // and (one level deep) whether THAT pointee holds a d3d12 object -> the ID3D12Resource path.
    for off in (0x00..=0xa0usize).step_by(0x08) {
        let p = read(hal + off);
        if p <= 0x10000 || p >= 0x8000_0000_0000 {
            continue;
        }
        let vt = read(p);
        let d = in_d3d(vt);
        // one hop deeper: does p hold a d3d12 object at p+0x08..p+0x40?
        let mut deep = String::new();
        for o2 in (0x08..=0x40usize).step_by(0x08) {
            let p2 = read(p + o2);
            if p2 > 0x10000 && p2 < 0x8000_0000_0000 {
                let vt2 = read(p2);
                if in_d3d(vt2) {
                    deep = format!(" | +0x{o2:02x}->0x{p2:x} vt_rva=0x{:x} IS_D3D", rva(vt2));
                    break;
                }
            }
        }
        append_autoload_debug(format_args!(
            "gfx-tex-dump: hal+0x{off:02x}=0x{p:x} vt_rva=0x{:x} in_d3d={d}{deep}",
            rva(vt)
        ));
    }
}

/// Version of `LOADING_BG_PORTRAIT_RGBA` last copied into the in-movie texture (usize::MAX = never), so
/// the copy runs once per fresh live head frame (the head tracks the look-at) without per-present churn.
static GFX_PORTRAIT_UPLOADED_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The displayed name currently cached in `GFX_PORTRAIT_CACHED_IMG`/`_HAL` (re-resolve on change only).
static GFX_PORTRAIT_CACHED_NAME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Per-frame (game thread): copy the live depth-keyed head into the DISPLAYED now-loading GFx texture so
/// the movie's native tips + Gauge_3 bar render above it. Resolves the displayed `CSTextureImage` by name
/// (cached across frames; re-resolved when the artwork rotates), then CopyTextureRegions the head
/// (resampled to the forged texture's own dims) into its HAL texture. On any success it refills the
/// Present-overlay demote credit so the overlay yields the head draw. Called from the game-thread save
/// task; runs the same D3D12 upload the proven one-shot used. Never panics.
pub(crate) unsafe fn maybe_update_gfx_loading_portrait(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // Release any image ref stranded by an off-thread window reset (Scaleform frees must run here).
    let orphan = GFX_PORTRAIT_ORPHAN_IMG.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        unsafe { release_gfx_texture_image(base, orphan) };
    }
    if !gfx_loading_portrait_enabled() {
        return;
    }
    // Need a captured head before there is anything to put in the movie.
    if PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) == 0 {
        return;
    }
    let Some(name) = (unsafe { current_displayed_menu_load_name(base) }) else {
        GFX_PORTRAIT_LAST_ERROR.store(GFX_PORTRAIT_ERR_NO_NAME, Ordering::SeqCst);
        return;
    };
    // Re-resolve only when the displayed artwork name changes; otherwise reuse the cached HAL texture.
    let mut name_guard = match GFX_PORTRAIT_CACHED_NAME.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if name_guard.as_deref() != Some(name.as_str()) {
        let old = GFX_PORTRAIT_CACHED_IMG.swap(0, Ordering::SeqCst);
        GFX_PORTRAIT_CACHED_HAL.store(0, Ordering::SeqCst);
        if old != 0 {
            unsafe { release_gfx_texture_image(base, old) };
        }
        match unsafe { resolve_displayed_gfx_texture(base, &name) } {
            Some((img, hal)) => {
                GFX_PORTRAIT_CACHED_IMG.store(img, Ordering::SeqCst);
                GFX_PORTRAIT_CACHED_HAL.store(hal, Ordering::SeqCst);
                *name_guard = Some(name);
                GFX_PORTRAIT_RESOLVES.fetch_add(1, Ordering::SeqCst);
            }
            None => {
                GFX_PORTRAIT_RESOLVE_FAILS.fetch_add(1, Ordering::SeqCst);
                return;
            }
        }
    }
    // Release the name lock BEFORE the (fence-waiting) upload so the render thread's window-reset never
    // blocks on it. The name we logged below is snapshotted first.
    let cached_name = name_guard.clone();
    drop(name_guard);
    let hal = GFX_PORTRAIT_CACHED_HAL.load(Ordering::SeqCst);
    if hal == 0 {
        return;
    }
    // One copy per fresh published head frame (version bump) so the in-movie head tracks the look-at.
    let cur_ver = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    if GFX_PORTRAIT_UPLOADED_VERSION.load(Ordering::SeqCst) == cur_ver {
        // Still refill the demote credit so the overlay keeps yielding even between head frames.
        GFX_PORTRAIT_DEMOTE_CREDIT.store(GFX_PORTRAIT_DEMOTE_REFILL, Ordering::SeqCst);
        return;
    }
    let Some((w, h, px)) = LOADING_BG_PORTRAIT_RGBA.lock().ok().and_then(|g| g.clone()) else {
        return;
    };
    if w == 0 || h == 0 {
        return;
    }
    match unsafe { upload_head_into_gfx_texture(hal, FORGE_HEAD_TEX_DIM, w, h, &px) } {
        Some((dw, dh)) => {
            GFX_PORTRAIT_UPLOADED_VERSION.store(cur_ver, Ordering::SeqCst);
            GFX_PORTRAIT_UPLOADS.fetch_add(1, Ordering::SeqCst);
            GFX_PORTRAIT_HAL_DIMS
                .store(((dw as usize) << 16) | (dh as usize), Ordering::SeqCst);
            GFX_PORTRAIT_DEMOTE_CREDIT.store(GFX_PORTRAIT_DEMOTE_REFILL, Ordering::SeqCst);
            GFX_PORTRAIT_LAST_ERROR.store(GFX_PORTRAIT_ERR_NONE, Ordering::SeqCst);
            if GFX_PORTRAIT_FIRST_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "gfx-loading-portrait: LIVE head copied INTO displayed now-loading GFx texture name={cached_name:?} hal=0x{hal:x} src={w}x{h} -> dst={dw}x{dh}; native tips/bar now render ABOVE the portrait (overlay demoting)"
                ));
            }
        }
        None => {
            GFX_PORTRAIT_LAST_ERROR.store(GFX_PORTRAIT_ERR_UPLOAD_FAILED, Ordering::SeqCst);
        }
    }
}

/// Reset candidate A state at loading-window end. Called from `loading_portrait_window_reset` (which runs
/// OFF the game thread), so the cached image ref is STASHED into the orphan slot for the game-thread
/// updater to Release on its next tick rather than freed here.
pub(crate) fn gfx_loading_portrait_window_reset() {
    GFX_PORTRAIT_DEMOTE_CREDIT.store(0, Ordering::SeqCst);
    GFX_PORTRAIT_UPLOADED_VERSION.store(usize::MAX, Ordering::SeqCst);
    GFX_PORTRAIT_CACHED_HAL.store(0, Ordering::SeqCst);
    let img = GFX_PORTRAIT_CACHED_IMG.swap(0, Ordering::SeqCst);
    if img != 0 {
        // Hand the ref to the game-thread updater to Release (rare double-reset may strand one ref until
        // process exit -- harmless; never freed off-thread).
        GFX_PORTRAIT_ORPHAN_IMG.store(img, Ordering::SeqCst);
    }
    if let Ok(mut g) = GFX_PORTRAIT_CACHED_NAME.lock() {
        *g = None;
    }
}
