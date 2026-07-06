
pub(crate) fn install_profile_select_table_diag_hook() {
    if PROFILE_SELECT_TABLE_DIAG_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "profileselect-table-diag: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_RENDERER_REFRESH_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            profile_select_table_diag_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_SELECT_TABLE_DIAG_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "profileselect-table-diag: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "profileselect-table-diag: MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            PROFILE_SELECT_TABLE_DIAG_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "profileselect-table-diag: hooked native profile builder 0x{target:x} (read-only table-state trace)"
            ));
        }
        status => append_autoload_debug(format_args!(
            "profileselect-table-diag: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

pub(crate) fn install_profile_renderer_teardown_spare_hook() {
    if PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "loading-portrait: teardown-spare MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_RENDERER_TEARDOWN_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            profile_renderer_teardown_spare_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_RENDERER_TEARDOWN_HOOK_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "loading-portrait: teardown-spare queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "loading-portrait: teardown-spare MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "loading-portrait: hooked profile-renderer teardown 0x{target:x} to spare slot0 for the now-loading portrait"
            ));
        }
        status => append_autoload_debug(format_args!(
            "loading-portrait: teardown-spare MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// Build a distinctive POC test-image TPF (magenta/yellow checker) whose single texture is named
/// EXACTLY `symbol`, so the CSScaleform pump's name-registration binds it to the now-loading image.
/// (Real loaded-character portrait pixels are a follow-up; this proves the injection + object shape.)
fn build_portrait_test_tpf(symbol: &str) -> Option<Vec<u8>> {
    // 1024x1024 to MATCH the captured menu-portrait dims, so once the real portrait is read back we can
    // D3D12-upload it straight into THIS (already-registered) displayed texture (the Scaleform pump binds
    // the name only on the first bind, so a re-forge can't swap it -- we overwrite the live pixels instead).
    let dds = er_tpf::DdsImage::checker(1024, 1024, 64, [255, 0, 255, 255], [255, 255, 0, 255])
        .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
    er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok()
}

/// Build the now-loading background TPF named exactly `symbol`. When `portrait_real_pixels_enabled()`
/// AND a live portrait readback is available (`LOADING_BG_PORTRAIT_RGBA` is `Some`), build the TPF
/// from the REAL rendered character-head RGBA8 pixels (uncompressed legacy-RGBA8 DDS). The engine
/// rebuilds a correct SRV from these bytes at `CreateTpfResCap` time -- the same mechanism that makes
/// the checker display correctly. Otherwise (default, or no capture yet) fall back to the proven
/// magenta/yellow checker, byte-for-byte unchanged.
/// THE SWAPPABLE LOADING-BACKGROUND LEVER (retained, NOT wired in by default). Building the TPF served here
/// replaces the game's `MENU_Load_*` now-loading background artwork. Currently a fully TRANSPARENT (RGBA
/// 0,0,0,0) 64x64 texture (Scaleform stretches it to fill; for a real image build at the native ~1024x1024
/// so it is not upscaled). PROVEN 2026-07-02 that the 3D world is NOT rendered during a map load, so a
/// transparent background reads BLACK, not passthrough. This is kept for when we deliberately want to
/// replace the loading background: call it from build_portrait_tpf on the desired path and install the forge
/// there. By default the stock artwork is left enabled (user choice).
#[allow(dead_code)]
fn build_loading_bg_replacement_tpf(symbol: &str) -> Option<Vec<u8>> {
    let dds = er_tpf::DdsImage {
        width: 64,
        height: 64,
        pixels: vec![0u8; 64 * 64 * 4],
    }
    .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
    er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok()
}

fn build_portrait_tpf(symbol: &str) -> Option<Vec<u8>> {
    // Candidate A (er-effects-rs-jsm): on the live-head path, BAKE the head into the forged now-loading
    // background image (the proven display path), built at the artwork's true 2:1 aspect with the
    // background + head aspect-cover CENTRE-CROPPED into the visible sub-rect -- never stretched. GFx shows
    // it in-movie under the native tips; the overlay demotes once a baked artwork is displayed.
    if gfx_loading_portrait_enabled() {
        return build_baked_loading_bg(symbol);
    }
    // Default product behavior: persist the chosen boot background (TOML override, explicit ERBGRA01
    // override, or latest local Steam screenshot) through the native MENU_Load_* GFX background too. This
    // keeps the pre-native boot screen and the game's loading screen visually continuous. Users can opt out
    // with `persist_boot_background_to_loading_screen = false` in DLL-adjacent er-effects.toml.
    if crate::config::persist_boot_background_to_loading_screen_enabled() {
        if let Some((w, h, px)) = boot_bg_image_rgba_clone() {
            let dds = er_tpf::DdsImage {
                width: w as u32,
                height: h as u32,
                pixels: px,
            }
            .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
            return er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok();
        }
    }
    // ONE-HEAD CONSOLIDATION: when the live build-own path is active, the present-overlay composite is the
    // SOLE deterministic display. Baking the real head into the forge TPF here produces a SECOND head (it
    // displays when the forge wins the bind race -- user-observed). So in render-drive mode the forge stays
    // a neutral checker background; the overlay draws the one head on top.
    if portrait_real_pixels_enabled() && !portrait_render_drive_enabled() {
        if let Ok(slot) = LOADING_BG_PORTRAIT_RGBA.lock() {
            if let Some((w, h, px)) = slot.as_ref() {
                let dds = er_tpf::DdsImage {
                    width: *w,
                    height: *h,
                    pixels: px.clone(),
                }
                .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
                return er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok();
            }
        }
    }
    build_portrait_test_tpf(symbol)
}

/// `FUN_140d69880` (deobf `LOADING_BG_REPLACE_BIND_RVA`) full-replace: the producer's "bind a
/// TpfFileCap to this rti from the symbol" step. For the now-loading background symbols
/// `MENU_Load_NNNNN`, build our own portrait TPF named exactly the symbol, materialize it through the
/// game's in-memory `CreateTpfResCap` factory, wrap it in a freshly-allocated `TpfFileCap`
/// (loadState=4), set it + the symbol on the rti, and return 1 -- so the producer lists the rti and
/// the unmodified per-frame CSScaleform pump registers our texture name, making GFx composite the
/// portrait as the loading-screen background. Every other symbol (and any build/alloc failure)
/// tail-calls the original, so the stock random background renders unchanged.
pub(crate) unsafe extern "system" fn loading_bg_replace_bind_hook(rti: usize, symbol: usize) -> u8 {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = LOADING_BG_TEXTURE_REDIRECT_ORIG.load(Ordering::SeqCst);
    let call_orig = move || -> u8 {
        if orig != null && orig != HOOK_ORIGINAL_UNSET {
            let f: unsafe extern "system" fn(usize, usize) -> u8 =
                unsafe { std::mem::transmute(orig) };
            unsafe { f(rti, symbol) }
        } else {
            0
        }
    };
    let total = LOADING_BG_REPLACE_BIND_TOTAL_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    // Fire on the portrait-lookat path too, not just product autoload: the native-continue smoke arms the
    // portrait via portrait_lookat_enabled() and does NOT set product_autoload_enabled() (observed pae=false
    // on the MENU_Load binds), so gating on pae alone never forged. Mirrors the teardown-spare gating fix.
    let pae = product_autoload_enabled() || portrait_lookat_enabled();
    let sym = unsafe { read_dlstring_u16(symbol) };
    // Diagnostic: log the first calls' symbols (ungated) so we can confirm whether the now-loading
    // MENU_Load_ background symbols actually reach this bind function and how they decode.
    if total <= 48 {
        let (preview, len) = match &sym {
            Some((u, _)) => (utf16_ascii_preview(u), u.len()),
            None => ("<read-fail>".to_string(), 0),
        };
        append_autoload_debug(format_args!(
            "loading-portrait-probe: call#{total} pae={pae} rti=0x{rti:x} symlen={len} sym='{preview}'"
        ));
    }
    if !pae || rti == 0 || rti == null {
        return call_orig();
    }
    let Some((units, encoding)) = sym else {
        return call_orig();
    };
    let Ok(sym_string) = String::from_utf16(&units) else {
        return call_orig();
    };
    // The producer symbol is a virtual TPF path, e.g. "menutpfbnd:/00_Solo/MENU_Load_00008.tpf".
    // Extract the bare GFx image symbol ("MENU_Load_00008"); skip anything that is not a now-loading
    // background.
    let Some(tex_name) = extract_menu_load_tex_name(&sym_string) else {
        return call_orig();
    };
    let attempts = LOADING_BG_TEXTURE_REDIRECT_ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
    let Ok(base) = game_module_base() else {
        return call_orig();
    };
    let Some(cap) = (unsafe { forge_into_rti(base, rti, &tex_name, encoding, symbol) }) else {
        return call_orig();
    };
    let commits = LOADING_BG_TEXTURE_REDIRECT_COMMITS.fetch_add(1, Ordering::SeqCst) + 1;
    LOADING_BG_TEXTURE_REDIRECT_LAST_SYMBOL_MATCH.store(1, Ordering::SeqCst);
    LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.store(cap, Ordering::SeqCst);
    // Remember the FIRST (displayed) rti + its name/encoding so we can RE-FORGE it once the real portrait
    // is baked (the sprite commits to this first bind, which happens before the portrait is captured).
    if LOADING_BG_FIRST_RTI
        .compare_exchange(0, rti, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        LOADING_BG_FIRST_ENCODING.store(encoding as usize, Ordering::SeqCst);
        if let Ok(mut g) = LOADING_BG_FIRST_TEX_NAME.lock() {
            *g = Some(tex_name.clone());
        }
    }
    let baked = LOADING_BG_PORTRAIT_RGBA
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false);
    if commits <= 8 {
        append_autoload_debug(format_args!(
            "loading-portrait: forged now-loading background symbol='{sym_string}' -> cap=0x{cap:x} baked_rgba={baked} tpf commits={commits} attempts={attempts}"
        ));
    }
    1
}

/// Build a now-loading TPF (baking LOADING_BG_PORTRAIT_RGBA if captured, else the checker), materialize it
/// through the game's in-memory CreateTpfResCap factory, wrap it in a fresh TpfFileCap, and bind it to
/// `rti`. `substr_symbol != 0` copies that DLString into the rti's symbol field (the initial forge);
/// pass 0 to leave the rti's existing symbol (a RE-FORGE of an already-bound rti). Returns the cap on
/// success. The PINs/refcount bump match the original forge so the CSScaleform GC can't free our graph.
unsafe fn forge_into_rti(
    base: usize,
    rti: usize,
    tex_name: &str,
    encoding: u8,
    substr_symbol: usize,
) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let name_z: Vec<u16> = tex_name.encode_utf16().chain(core::iter::once(0)).collect();
    let tpf_bytes = build_portrait_tpf(tex_name)?;
    let tpf_repo = unsafe { safe_read_usize(base + GLOBAL_TPF_REPOSITORY_RVA) }.unwrap_or(0);
    if tpf_repo == 0 {
        return None;
    }
    let create_rescap: unsafe extern "system" fn(
        usize,
        *const u16,
        *const u8,
        u64,
        u8,
        u32,
    ) -> usize = unsafe { std::mem::transmute(base + CREATE_TPF_RESCAP_RVA) };
    let container = unsafe {
        create_rescap(
            tpf_repo,
            name_z.as_ptr(),
            tpf_bytes.as_ptr(),
            tpf_bytes.len() as u64,
            0,
            0,
        )
    };
    if container == 0 || container == null {
        return None;
    }
    let main_heap = unsafe { safe_read_usize(base + GLOBAL_MAIN_HEAP_ALLOCATOR_RVA) }.unwrap_or(0);
    if main_heap == 0 {
        return None;
    }
    let heap_alloc: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + GAME_HEAP_ALLOC_RVA) };
    let cap = unsafe { heap_alloc(TPF_FILE_CAP_ALLOC_SIZE, TPF_FILE_CAP_ALLOC_ALIGN, main_heap) };
    if cap == 0 {
        return None;
    }
    let cap_ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + TPF_FILE_CAP_CTOR_RVA) };
    unsafe { cap_ctor(cap, 0) };
    unsafe {
        ((cap + TPF_FILE_CAP_LOAD_STATE_OFFSET) as *mut u8)
            .write_volatile(TPF_FILE_CAP_LOADED_STATE)
    };
    let prev_flags = unsafe { safe_read_u8(cap + TPF_FILE_CAP_FLAGS_OFFSET) }.unwrap_or(0);
    unsafe {
        ((cap + TPF_FILE_CAP_FLAGS_OFFSET) as *mut u8)
            .write_volatile(prev_flags | TPF_FILE_CAP_READY_FLAG_BIT)
    };
    unsafe { ((cap + TPF_FILE_CAP_TEX_RESCAP_OFFSET) as *mut usize).write_volatile(container) };
    unsafe { ((rti + REPLACE_TEX_INFO_ENCODING_OFFSET) as *mut u8).write_volatile(encoding) };
    if substr_symbol != 0 {
        let substr: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(base + DLSTRING_WCHAR_SUBSTR_RVA) };
        unsafe {
            substr(
                rti + REPLACE_TEX_INFO_SYMBOL_OFFSET,
                substr_symbol,
                0,
                usize::MAX,
            )
        };
    }
    unsafe { ((rti + REPLACE_TEX_INFO_TPF_FILE_CAP_OFFSET) as *mut usize).write_volatile(cap) };
    unsafe { ((rti + REPLACE_TEX_INFO_READY_OFFSET) as *mut u8).write_volatile(0) };
    let rc = unsafe {
        &*((rti + REPLACE_TEX_INFO_REFCOUNT_OFFSET) as *const core::sync::atomic::AtomicI32)
    };
    rc.fetch_add(0x10000, Ordering::SeqCst);
    Some(cap)
}

/// Build (once, cached for the process lifetime) the neutral-background TPF003 blob for a stats-panel
/// slot: a solid `STATS_PANEL_BG_RGBA` `STATS_PANEL_TEX_DIM` square, uncompressed legacy-RGBA8 DDS,
/// wrapped in a one-entry TPF whose ENTRY NAME == the slot's `STATS_PANEL_SYSTEX_KEYS` (which becomes
/// the GLOBAL_TexRepository GPU key). Held alive forever so the engine's DEFERRED GPU upload can never
/// read freed bytes (same lifetime discipline the er-tpf cover used). Pure CPU; no native call, no disk.
fn stats_panel_tpf_blob(slot: usize) -> Option<&'static [u8]> {
    static BLOBS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    let blobs = BLOBS.get_or_init(|| {
        (0..STATS_PANEL_SLOT_COUNT)
            .map(|s| {
                let img = er_tpf::DdsImage::solid(
                    STATS_PANEL_TEX_DIM,
                    STATS_PANEL_TEX_DIM,
                    STATS_PANEL_BG_RGBA,
                );
                let dds = img.to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
                er_tpf::Tpf::single_pc(STATS_PANEL_SYSTEX_KEYS[s], dds, 1)
                    .build()
                    .unwrap_or_default()
            })
            .collect()
    });
    match blobs.get(slot) {
        Some(b) if !b.is_empty() => Some(b.as_slice()),
        _ => None,
    }
}

/// Stats-panel product mode: register the neutral-background texture for each ProfileSelect save slot
/// under its unique `STATS_PANEL_SYSTEX_KEYS` via the engine's own in-memory `CS::CreateTpfResCap`
/// factory -- the SAME proven raw-(ptr,len) TPF->GPU path the er-tpf cover and the now-loading forge
/// use. Self-gating + fail-closed: runs on the CSTaskImp game task (post-gfx-init), validates every
/// precondition before the first native call, wraps each call in `catch_unwind`, and only latches a
/// slot's registered bit on a non-null TpfResCap -- so a not-yet-initialized repo (null during boot)
/// simply retries next tick and never crashes. Idempotent per slot via `STATS_PANEL_TEX_REGISTERED_MASK`.
/// The visible-surface redirect is a separate step in the Scaleform bind observer, gated on each slot's
/// registered bit. A texture upload is cheap (no per-frame render), so all 10 slots register with no
/// GX-queue cost -- unlike driving 10 concurrent CSMenuProfModelRend renderers (the 0x1aeaf05 crash).
pub(crate) unsafe fn maybe_register_stats_panel_textures(base: usize) {
    if !stats_panel_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_BASE_UNRESOLVED, Ordering::SeqCst);
        return;
    }
    let all: usize = (1 << STATS_PANEL_SLOT_COUNT) - 1;
    if STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst) & all == all {
        return; // every slot already registered
    }
    // Both repos non-null == graphics/repos initialized. Bail (retry next tick) if not ready yet; do
    // NOT consume any register attempt, so boot-time nulls never burn a slot.
    let tpf_repo = unsafe { safe_read_usize(base + GLOBAL_TPF_REPOSITORY_RVA) }.unwrap_or(0);
    if tpf_repo == 0 || tpf_repo == null {
        STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_TPF_REPO_NULL, Ordering::SeqCst);
        return;
    }
    let tex_repo = unsafe { safe_read_usize(base + GLOBAL_TEX_REPOSITORY_RVA) }.unwrap_or(0);
    if tex_repo == 0 || tex_repo == null {
        STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_TEX_REPO_NULL, Ordering::SeqCst);
        return;
    }
    let create_rescap: unsafe extern "system" fn(
        usize,
        *const u16,
        *const u8,
        u64,
        u8,
        u32,
    ) -> usize = unsafe { std::mem::transmute(base + CREATE_TPF_RESCAP_RVA) };
    for slot in 0..STATS_PANEL_SLOT_COUNT {
        if STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst) & (1 << slot) != 0 {
            continue;
        }
        let Some(tpf_bytes) = stats_panel_tpf_blob(slot) else {
            STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_BLOB_EMPTY, Ordering::SeqCst);
            continue;
        };
        let name_z: Vec<u16> = STATS_PANEL_SYSTEX_KEYS[slot]
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();
        STATS_PANEL_TEX_REGISTER_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        let ptr = tpf_bytes.as_ptr();
        let len = tpf_bytes.len() as u64;
        let container = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            create_rescap(tpf_repo, name_z.as_ptr(), ptr, len, 0, 0)
        }));
        match container {
            Ok(c) if c != 0 && c != null => {
                STATS_PANEL_TEX_REGISTERED_MASK.fetch_or(1 << slot, Ordering::SeqCst);
                // Clear the stale boot-time retry marker (repos were null before gfx came up, which set
                // TPF_REPO_NULL); a real register succeeded, so the oracle should read NONE.
                STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_NONE, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "stats-panel: registered neutral bg for slot {slot} key='{}' rescap=0x{c:x} (mask=0x{:x})",
                    STATS_PANEL_SYSTEX_KEYS[slot],
                    STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst)
                ));
            }
            Ok(_) => {
                STATS_PANEL_TEX_REGISTER_FAILURES.fetch_add(1, Ordering::SeqCst);
                STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_RESCAP_NULL, Ordering::SeqCst);
            }
            Err(_) => {
                STATS_PANEL_TEX_REGISTER_FAILURES.fetch_add(1, Ordering::SeqCst);
                STATS_PANEL_LAST_ERROR.store(STATS_PANEL_ERR_PANIC, Ordering::SeqCst);
            }
        }
    }
}

/// Parse the trailing 2-digit slot index (`00`..`09`) from a `systex_menu_profileNN` target DLString.
/// Returns `Some(0..=9)` only for a target that actually looks like the profile SYSTEX key, else `None`
/// (so we never redirect the status-face / kick-face / decorative binds).
unsafe fn systex_profile_target_slot(target_ptr: usize) -> Option<usize> {
    let mut buf = [0u8; 96];
    let n = unsafe { copy_ascii_preview(target_ptr, &mut buf) };
    if n < 2 {
        return None;
    }
    let s = &buf[..n];
    // Lowercase compare against the known prefix so casing never matters.
    let mut lower = [0u8; 96];
    for (i, b) in s.iter().enumerate() {
        lower[i] = b.to_ascii_lowercase();
    }
    let lower = &lower[..n];
    if !lower
        .windows(b"systex_menu_profile".len())
        .any(|w| w == b"systex_menu_profile")
    {
        return None;
    }
    let d1 = s[n - 2];
    let d0 = s[n - 1];
    if !d1.is_ascii_digit() || !d0.is_ascii_digit() {
        return None;
    }
    let slot = ((d1 - b'0') as usize) * 10 + (d0 - b'0') as usize;
    if slot < STATS_PANEL_SLOT_COUNT {
        Some(slot)
    } else {
        None
    }
}

/// Once the real portrait is baked into LOADING_BG_PORTRAIT_RGBA, OVERWRITE the displayed now-loading
/// background texture's PIXELS in place via D3D12 upload. Re-forging a new cap doesn't work -- the
/// Scaleform pump registers the texture by NAME only on the first bind and won't re-read a swapped cap --
/// so we keep the first forged (1024x1024 checker) texture the pump already bound and just replace its
/// pixels with the captured 1024 portrait. One-shot. Render/D3D12 work mirrors the slot-dump readback
/// which runs safely from this same game-thread task.
pub(crate) unsafe fn maybe_reforge_loading_portrait(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    if PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) == 0 {
        return;
    }
    // LIVE RE-UPLOAD (version-gated): re-upload the displayed now-loading texture whenever the live feed
    // publishes a new frame (version advanced) so the loading-screen head TRACKS the look-at. The earlier
    // re-upload crash was the READBACK's D3D12 object SCAN racing teardown (now fixed: cached resource, no
    // re-scan); the upload writes into the already-bound, stable now-loading texture (the one-shot upload
    // persisted crash-free through the whole loading screen), so per-version re-upload is safe.
    let cur_ver = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    if LOADING_BG_REFORGE_VERSION.load(Ordering::SeqCst) == cur_ver {
        return; // already uploaded this frame's content
    }
    // The DISPLAYED texture is the one the Scaleform sprite samples by NAME from GLOBAL_TexRepository --
    // NOT the forge's source container GX. Look it up: GetResCap(GLOBAL_TexRepository, name).gxTexture.
    let tex_repo = unsafe { safe_read_usize(base + GLOBAL_TEX_REPOSITORY_RVA) }.unwrap_or(0);
    if !valid(tex_repo) {
        return;
    }
    let tex_name = match LOADING_BG_FIRST_TEX_NAME.lock() {
        Ok(g) => match g.as_ref() {
            Some(s) => s.clone(),
            None => return,
        },
        Err(_) => return,
    };
    let name_z: Vec<u16> = tex_name.encode_utf16().chain(core::iter::once(0)).collect();
    let get_res_cap: unsafe extern "system" fn(usize, *const u16) -> usize =
        unsafe { std::mem::transmute(base + TEX_REPOSITORY_GET_RES_CAP_RVA) };
    let res_cap = unsafe { get_res_cap(tex_repo, name_z.as_ptr()) };
    if !valid(res_cap) {
        return;
    }
    let gx = unsafe { safe_read_usize(res_cap + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
        .unwrap_or(0);
    if !valid(gx) {
        return;
    }
    // Snapshot the captured portrait pixels.
    let snapshot = match LOADING_BG_PORTRAIT_RGBA.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    let Some((w, h, px)) = snapshot else {
        return;
    };
    let ok = unsafe { upload_rgba_to_texture(gx, w, h, &px) };
    // VERIFY: read back the SAME gx right after the upload. If it now reads the portrait, the upload DID
    // land in this texture (so any remaining checker on screen means Scaleform samples a DIFFERENT copy);
    // if it still reads the checker (bright magenta/yellow, rgb~255 with high variance), find_d3d12_resource
    // picked the wrong same-size texture and the upload missed -> fixable by targeting deterministically.
    let mut verify_rgb = (0u8, 0u8, 0u8);
    if let Some((vw, vh, vpx)) = unsafe { readback_offscreen_rgba8(gx) } {
        let n = (vw as usize) * (vh as usize);
        if vpx.len() >= n * 4 {
            let (cx, cy) = (vw as usize / 2, vh as usize / 2);
            let idx = (cy * vw as usize + cx) * 4;
            verify_rgb = (vpx[idx], vpx[idx + 1], vpx[idx + 2]);
        }
    }
    // Advance the version latch ONLY on a successful upload (dims matched). On failure we leave the latch
    // behind so a later same-version frame can retry once -- but since the version only advances when the
    // live feed publishes a NEW frame, this never per-frame-hammers (the old dim-mismatch crash). Same dims
    // (1024) throughout the build-own path, so ok is reliably true.
    if ok {
        LOADING_BG_REFORGE_VERSION.store(cur_ver, Ordering::SeqCst);
    }
    if LOADING_BG_REFORGE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "loading-portrait: UPLOADED real portrait {w}x{h} into displayed now-loading texture gx=0x{gx:x} ok={ok} verify_center_rgb=({},{},{}) (loading screen now shows the LIVE character, re-uploads per version)",
            verify_rgb.0, verify_rgb.1, verify_rgb.2
        ));
    }
    let _ = base;
}

pub(crate) fn install_loading_bg_replace_bind_hook() {
    if LOADING_BG_TEXTURE_REDIRECT_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "loading-portrait: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(LOADING_BG_REPLACE_BIND_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            loading_bg_replace_bind_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            LOADING_BG_TEXTURE_REDIRECT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "loading-portrait: queue_enable failed for replace-bind 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "loading-portrait: replace-bind MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            LOADING_BG_TEXTURE_REDIRECT_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "loading-portrait: hooked now-loading replace-bind 0x{target:x}; will forge a portrait TPF for {LOADING_BG_SYMBOL_PREFIX}NNNNN backgrounds under product autoload"
            ));
        }
        status => append_autoload_debug(format_args!(
            "loading-portrait: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn title_menu_resource_acquire_observer_hook(
    this: usize,
    load_params: usize,
    param3: u8,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let filename_ptr = if load_params != 0 && load_params != null {
        unsafe { safe_read_usize(load_params + 0x8) }.unwrap_or(null)
    } else {
        null
    };
    let hit = TITLE_MENU_RESOURCE_ACQUIRE_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_THIS.store(this, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_LOAD_PARAMS.store(load_params, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_FILENAME_PTR.store(filename_ptr, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_PARAM3.store(param3 as usize, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { wide_ascii_contains_ci(filename_ptr, b"05_001_title_logo") }
        || unsafe { wide_ascii_contains_ci(filename_ptr, b"05_001_title") };

    let orig = TITLE_MENU_RESOURCE_ACQUIRE_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u8) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, load_params, param3) }
    } else {
        null
    };
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_RET.store(ret, Ordering::SeqCst);

    if is_title_logo {
        let logo_hit = TITLE_MENU_RESOURCE_ACQUIRE_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_wide_ascii_preview(filename_ptr, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: AcquireMenuResource title-logo hit={logo_hit} total={hit} this=0x{this:x} load_params=0x{load_params:x} filename_ptr=0x{filename_ptr:x} filename='{name}' param3={param3} ret=0x{ret:x} caller_rva=0x{caller_rva:x}; observe-only"
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_wide_ascii_preview(filename_ptr, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: AcquireMenuResource sample total={hit} filename='{name}' ret=0x{ret:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

unsafe fn construct_title_scaleform_memory_file(
    base: usize,
    url: usize,
    bytes: &[u8],
) -> Option<usize> {
    if bytes.is_empty() || bytes.len() > u32::MAX as usize {
        return None;
    }
    let memory_global = unsafe { safe_read_usize(base + SCALEFORM_MEMORY_GLOBAL_RVA) }?;
    let memory_vtable = unsafe { safe_read_usize(memory_global) }?;
    let alloc_fn = unsafe { safe_read_usize(memory_vtable + 0x50) }?;
    if alloc_fn == 0 || alloc_fn == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let alloc: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(alloc_fn) };
    let file = unsafe { alloc(memory_global, SCALEFORM_MEMORY_FILE_SIZE, 0) };
    if file == 0 || file == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let dlstring_copy: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + SCALEFORM_DLSTRING_CHAR_COPY_RVA) };
    unsafe {
        core::ptr::write(file as *mut usize, base + SCALEFORM_MEMORY_FILE_VTABLE_RVA);
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_REFCOUNT_OFFSET) as *mut u32,
            1,
        );
        dlstring_copy(file + SCALEFORM_MEMORY_FILE_NAME_OFFSET, url);
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            bytes.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            bytes.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_VALID_OFFSET) as *mut u8, 1);
    }
    Some(file)
}

/// Product-default 05_000_title strip WITHOUT embedded bytes (er-effects-rs-h7x). `file` is what
/// the native FileOpener just returned for `data0:/menu/05_000_title.gfx`; per the rescap static
/// RE (`FUN_140ce8320`, bd `native-memoryfile-wrapper-expects-gfx-rescap-2026-06-28`) that is a
/// Scaleform MemoryFile whose data/len fields point at the vanilla movie payload owned by
/// `GLOBAL_GfxRepository` (the file object never frees the payload -- the proven synthetic
/// construct path already relied on that). Derive the stripped movie from that payload with
/// `er_gfx::title_05_000::strip` (all-or-nothing content-addressed edits, output verified against
/// the validated-asset fingerprint for the known vanilla input), cache it for the process
/// lifetime, and swap the native file's data/len/cursor onto the cached buffer. ANY failure
/// leaves the native file untouched and returns it as-is: fail-closed to the vanilla title UI,
/// never a crash, never a half-stripped movie.
unsafe fn title_05_000_swap_to_stripped(base: usize, file: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if file == 0 || file == null || file == HOOK_ORIGINAL_UNSET {
        return false;
    }
    let fail = |reason: core::fmt::Arguments<'_>| {
        TITLE_05_000_RUNTIME_STRIP_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "title-resource-observer: 05_000 runtime strip FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!(
            "unexpected file vtable 0x{vtable:x} (want MemoryFile 0x{:x})",
            base + SCALEFORM_MEMORY_FILE_VTABLE_RVA
        ));
    }
    let stripped = match TITLE_05_000_RUNTIME_STRIPPED.get() {
        Some(cached) => cached,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == null || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            // Probe both ends through the guarded reader before the bulk copy; the payload is one
            // contiguous repository allocation, so readable ends imply a readable middle.
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} is unreadable or not GFX-magic"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            TITLE_05_000_RUNTIME_STRIP_INPUT_LEN.store(len, Ordering::SeqCst);
            let known = er_gfx::title_05_000::is_known_vanilla(vanilla);
            TITLE_05_000_RUNTIME_STRIP_INPUT_CLASS
                .store(if known { 1 } else { 2 }, Ordering::SeqCst);
            match er_gfx::title_05_000::strip(vanilla) {
                Ok(out) => {
                    TITLE_05_000_RUNTIME_STRIP_OUTPUT_LEN.store(out.len(), Ordering::SeqCst);
                    let validated = out.len() == er_gfx::title_05_000::STRIPPED_LEN
                        && er_gfx::title_05_000::fnv1a64(&out)
                            == er_gfx::title_05_000::STRIPPED_FNV1A64;
                    TITLE_05_000_RUNTIME_STRIP_OUTPUT_VALIDATED
                        .store(if validated { 1 } else { 2 }, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-resource-observer: 05_000 runtime strip derived in={len} out={} known_vanilla={known} out_fnv=0x{:016x}",
                        out.len(),
                        er_gfx::title_05_000::fnv1a64(&out)
                    ));
                    TITLE_05_000_RUNTIME_STRIPPED.get_or_init(|| out)
                }
                Err(err) => {
                    return fail(format_args!("in={len} known_vanilla={known}: {err}"));
                }
            }
        }
    };
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            stripped.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            stripped.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    TITLE_05_000_RUNTIME_STRIP_SERVES.fetch_add(1, Ordering::SeqCst);
    // Keep the established product-strip oracles counting regardless of mechanism (the
    // construct-from-embedded path incremented both of these).
    TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
    TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
    TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE.store(file, Ordering::SeqCst);
    true
}

/// Stats-panel 05_010_ProfileSelect runtime edit (mirrors `title_05_000_swap_to_stripped`): derive
/// the stats-panel movie (face box removed, `ErStats` field added, left column reflowed -- see
/// `er_gfx::title_05_010`) from the native MemoryFile's own vanilla payload, cache it for the
/// process lifetime, and swap the native file's data/len/cursor onto the cached buffer. ANY failure
/// leaves the native file untouched and returns it as-is: fail-closed to the vanilla ProfileSelect
/// rows (the row-populate hook's push then fails cleanly on the missing field), never a crash,
/// never a half-edited movie.
unsafe fn profile_05_010_swap_to_edited(base: usize, file: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if file == 0 || file == null || file == HOOK_ORIGINAL_UNSET {
        return false;
    }
    let fail = |reason: core::fmt::Arguments<'_>| {
        PROFILE_05_010_RUNTIME_EDIT_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "stats-panel: 05_010 runtime edit FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!(
            "unexpected file vtable 0x{vtable:x} (want MemoryFile 0x{:x})",
            base + SCALEFORM_MEMORY_FILE_VTABLE_RVA
        ));
    }
    let edited = match PROFILE_05_010_RUNTIME_EDITED.get() {
        Some(cached) => cached,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == null || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} is unreadable or not GFX-magic"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            PROFILE_05_010_RUNTIME_EDIT_INPUT_LEN.store(len, Ordering::SeqCst);
            let known = er_gfx::title_05_010::is_known_vanilla(vanilla);
            PROFILE_05_010_RUNTIME_EDIT_INPUT_CLASS
                .store(if known { 1 } else { 2 }, Ordering::SeqCst);
            match er_gfx::title_05_010::stats_panel(vanilla) {
                Ok(out) => {
                    PROFILE_05_010_RUNTIME_EDIT_OUTPUT_LEN.store(out.len(), Ordering::SeqCst);
                    let validated = out.len() == er_gfx::title_05_010::EDITED_LEN
                        && er_gfx::title_05_000::fnv1a64(&out)
                            == er_gfx::title_05_010::EDITED_FNV1A64;
                    PROFILE_05_010_RUNTIME_EDIT_OUTPUT_VALIDATED
                        .store(if validated { 1 } else { 2 }, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "stats-panel: 05_010 runtime edit derived in={len} out={} known_vanilla={known} out_fnv=0x{:016x}",
                        out.len(),
                        er_gfx::title_05_000::fnv1a64(&out)
                    ));
                    PROFILE_05_010_RUNTIME_EDITED.get_or_init(|| out)
                }
                Err(err) => {
                    return fail(format_args!("in={len} known_vanilla={known}: {err}"));
                }
            }
        }
    };
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            edited.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            edited.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    PROFILE_05_010_RUNTIME_EDIT_SERVES.fetch_add(1, Ordering::SeqCst);
    true
}

pub(crate) unsafe extern "system" fn title_scaleform_file_open_observer_hook(
    loader: usize,
    url: usize,
    flags: u32,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let hit = TITLE_SCALEFORM_FILE_OPEN_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_SCALEFORM_FILE_OPEN_LAST_LOADER.store(loader, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_URL_PTR.store(url, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_FLAGS.store(flags as usize, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { bounded_ascii_contains(url, b"05_001_title_logo") }
        || unsafe { bounded_ascii_contains(url, b"05_001_title") };
    let is_title_05_000 = unsafe { bounded_ascii_contains(url, b"05_000_title") };
    let is_profile_05_010 = unsafe { bounded_ascii_contains(url, b"05_010_profileselect") };

    let base = game_module_base().unwrap_or(null);
    let mut memory_replacement = false;
    let mut memory_label = "";
    let memory_bytes = if is_title_logo {
        memory_label = "05_001_title_logo";
        TITLE_SCALEFORM_MEMORY_GFX.get().map(Vec::as_slice)
    } else if is_title_05_000 {
        memory_label = "05_000_title";
        TITLE_SCALEFORM_05_000_MEMORY_GFX.get().map(Vec::as_slice)
    } else if is_profile_05_010 {
        // No embedded/env-loaded movie for 05_010: only the in-place runtime edit above.
        memory_label = "05_010_profileselect";
        None
    } else {
        None
    };
    let orig = TITLE_SCALEFORM_FILE_OPEN_ORIG.load(Ordering::SeqCst);
    let ret = if base != null {
        if let Some(bytes) = memory_bytes {
            match unsafe { construct_title_scaleform_memory_file(base, url, bytes) } {
                Some(file) => {
                    memory_replacement = true;
                    TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
                    if is_title_05_000 {
                        TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS
                            .fetch_add(1, Ordering::SeqCst);
                    }
                    TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE.store(file, Ordering::SeqCst);
                    file
                }
                None => {
                    TITLE_SCALEFORM_MEMORY_GFX_FAILURES.fetch_add(1, Ordering::SeqCst);
                    if orig != null && orig != HOOK_ORIGINAL_UNSET {
                        let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
                            unsafe { std::mem::transmute(orig) };
                        unsafe { f(loader, url, flags) }
                    } else {
                        null
                    }
                }
            }
        } else if orig != null && orig != HOOK_ORIGINAL_UNSET {
            let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
                unsafe { std::mem::transmute(orig) };
            let native = unsafe { f(loader, url, flags) };
            // Product-default runtime strip (er-effects-rs-h7x): derive the stripped title
            // movie from the native file's own vanilla payload and swap it in place. On any
            // failure the untouched native file is returned (vanilla title UI, fail-closed).
            if is_title_05_000 && TITLE_05_000_RUNTIME_STRIP_ARMED.load(Ordering::SeqCst) != 0 {
                memory_replacement = unsafe { title_05_000_swap_to_stripped(base, native) };
            }
            // Stats-panel 05_010 edit: same in-place derive-and-swap, same fail-closed shape.
            if is_profile_05_010 && PROFILE_05_010_RUNTIME_EDIT_ARMED.load(Ordering::SeqCst) != 0 {
                memory_replacement = unsafe { profile_05_010_swap_to_edited(base, native) };
            }
            native
        } else {
            null
        }
    } else if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(loader, url, flags) }
    } else {
        null
    };
    let ret_vtable = if ret != null && ret != HOOK_ORIGINAL_UNSET {
        unsafe { safe_read_usize(ret) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_SCALEFORM_FILE_OPEN_LAST_RET.store(ret, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_RET_VTABLE.store(ret_vtable, Ordering::SeqCst);

    // Capture the game's menu font (font:/<locale>/font.gfx) for our loading-screen stats text (read-only
    // copy of the file's own GFX payload; er-effects-rs-jsm). Observe-only, one-shot.
    if base != null
        && (unsafe { bounded_ascii_contains(url, b"font.gfx") }
            || unsafe { bounded_ascii_contains(url, b"font.swf") })
    {
        unsafe { capture_menu_font_gfx(base, ret) };
    }

    if is_title_logo || is_title_05_000 || is_profile_05_010 {
        let logo_hit = if is_title_logo {
            TITLE_SCALEFORM_FILE_OPEN_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            0
        };
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform file-open title-memory label={memory_label} logo_hit={logo_hit} total={hit} loader=0x{loader:x} url=0x{url:x} '{name}' flags=0x{flags:x} ret=0x{ret:x} ret_vtable=0x{ret_vtable:x} caller_rva=0x{caller_rva:x} memory_replacement={memory_replacement} total_memory_bytes={}",
            TITLE_SCALEFORM_MEMORY_GFX_BYTES.load(Ordering::SeqCst)
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform file-open sample total={hit} url='{name}' flags=0x{flags:x} ret=0x{ret:x} ret_vtable=0x{ret_vtable:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn title_scaleform_resource_ctor_observer_hook(
    out_resource: usize,
    loader_data: usize,
    file_type: u32,
    url: usize,
    file_obj: usize,
    external_flag: u8,
    heap_arg: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let hit = TITLE_SCALEFORM_RESOURCE_CTOR_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_OUT.store(out_resource, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_URL_PTR.store(url, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_FILE.store(file_obj, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { bounded_ascii_contains(url, b"05_001_title_logo") }
        || unsafe { bounded_ascii_contains(url, b"05_001_title") };

    let orig = TITLE_SCALEFORM_RESOURCE_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32, usize, usize, u8, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe {
            f(
                out_resource,
                loader_data,
                file_type,
                url,
                file_obj,
                external_flag,
                heap_arg,
            )
        }
    } else {
        null
    };
    let movie_data = if ret != null && ret != HOOK_ORIGINAL_UNSET {
        unsafe { safe_read_usize(ret + 0x40) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_RET.store(ret, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_MOVIE_DATA.store(movie_data, Ordering::SeqCst);

    if is_title_logo {
        let logo_hit = TITLE_SCALEFORM_RESOURCE_CTOR_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform resource-ctor title-logo hit={logo_hit} total={hit} out=0x{out_resource:x} url=0x{url:x} '{name}' file=0x{file_obj:x} file_type={file_type} external_flag={external_flag} ret=0x{ret:x} movie_data=0x{movie_data:x} caller_rva=0x{caller_rva:x}; observe-only"
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform resource-ctor sample total={hit} url='{name}' file=0x{file_obj:x} file_type={file_type} ret=0x{ret:x} movie_data=0x{movie_data:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}
